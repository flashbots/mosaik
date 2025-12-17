use {
	super::{
		Catalog,
		Config,
		Error,
		Event,
		PeerEntry,
		announce::{self, Announce},
		catalog::UpsertResult,
		sync::CatalogSync,
	},
	crate::{
		network::LocalNode,
		primitives::{IntoIterOrSingle, Short},
	},
	core::iter::once,
	futures::StreamExt,
	iroh::{
		EndpointAddr,
		Watcher,
		discovery::{Discovery, static_provider::StaticProvider},
		endpoint::Connection,
		protocol::DynProtocolHandler,
	},
	std::{io, sync::Arc},
	tokio::{
		sync::{
			broadcast,
			mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
			oneshot,
			watch,
		},
		task::JoinSet,
	},
};

/// Discovery worker handle
///
/// This struct provides an interface to interact with the discovery worker
/// loop. It can be used to send commands or queries to the worker.
///
/// This struct is instantiated by the `WorkerLoop::spawn` method and is held
/// by the `Discovery` struct.
pub(super) struct Handle {
	pub local: LocalNode,
	pub catalog: watch::Sender<Catalog>,
	pub events: broadcast::Receiver<Event>,
	pub commands: UnboundedSender<WorkerCommand>,
}

impl Handle {
	/// Sends a command to dial a peer with the given `PeerId`
	pub async fn dial<V>(&self, peers: impl IntoIterOrSingle<EndpointAddr, V>) {
		let (tx, rx) = oneshot::channel();
		self
			.commands
			.send(WorkerCommand::DialPeers(
				peers.iterator().into_iter().collect(),
				tx,
			))
			.ok();
		let _ = rx.await;
	}

	/// Updates the local peer entry using the provided update function.
	///
	/// If the update results in a change to the local entry contents, it is
	/// re-signed and broadcasted to the network which respectively updates their
	/// catalogues.
	///
	/// This api is not intended to be used directly by users of the discovery
	/// system, but rather by higher-level abstractions that manage the local
	/// peer's state.
	pub(crate) fn update_local_entry(
		&self,
		update: impl FnOnce(PeerEntry) -> PeerEntry + Send + 'static,
	) {
		self.catalog.send_modify(|catalog| {
			let local_entry = catalog.local().clone();
			let updated_entry = update(local_entry.into());
			let signed_updated_entry = updated_entry
				.sign(self.local.secret_key())
				.expect("signing updated local peer entry failed.");

			tracing::trace!(
				info = %Short(&signed_updated_entry),
				network = %self.local.network_id(),
				"Updating local peer entry in catalog",
			);

			assert!(
				catalog.upsert_signed(signed_updated_entry).is_ok(),
				"local peer info versioning error. this is a bug."
			);
		});
	}
}

/// Discovery background worker loop
///
/// This is a long-running task that is owned by the [`Discovery`] struct that
/// is responsible for continuously running the discovery protocols and handling
/// incoming events.
///
/// Interactions with the worker loop are done through the [`Handle`] struct.
pub(super) struct WorkerLoop {
	/// Allows the public API to interact with the internal worker loop.
	handle: Arc<Handle>,

	/// Full catalog sync protocol handler.
	sync: CatalogSync,

	/// Peer gossip announcement protocol handler.
	announce: Announce,

	/// Public API discovery events.
	events: broadcast::Sender<Event>,

	/// Ongoing catalog sync tasks.
	syncs: JoinSet<Result<(), Error>>,

	/// Incoming commands from the public API.
	commands: UnboundedReceiver<WorkerCommand>,

	/// Static addressing provider that publishes discovered peer addresses to
	/// iroh endpoint addressing resolver.
	provider: StaticProvider,
}

impl WorkerLoop {
	/// Constructs a new discovery worker loop and spawns it as a background task.
	/// Returns a handle to interact with the worker loop. This is called from
	/// the `Discovery::new` method.
	#[expect(clippy::needless_pass_by_value)]
	pub(super) fn spawn(local: LocalNode, config: Config) -> Arc<Handle> {
		let catalog = watch::Sender::new(Catalog::new(&local, &config));
		let (commands_tx, commands_rx) = unbounded_channel();
		let (events_tx, events_rx) = broadcast::channel(config.events_backlog);

		let sync = CatalogSync::new(local.clone(), catalog.clone());
		let announce = Announce::new(local.clone(), &config, catalog.subscribe());

		let handle = Arc::new(Handle {
			local,
			catalog,
			events: events_rx,
			commands: commands_tx,
		});

		let worker = Self {
			handle: Arc::clone(&handle),
			sync,
			announce,
			events: events_tx,
			syncs: JoinSet::new(),
			commands: commands_rx,
			provider: StaticProvider::new(),
		};

		// spawn the worker loop
		tokio::spawn(async move {
			let local = worker.handle.local.clone();
			let result = worker.run().await;

			if let Err(ref e) = result {
				tracing::error!(
					error = %e,
					network = %local.network_id(),
					"Discovery worker loop terminated with error"
				);
			}

			// initiate network shutdown
			local.termination().cancel();

			result
		});

		// Return the handle to interact with the worker loop
		handle
	}
}

impl WorkerLoop {
	async fn run(mut self) -> Result<(), Error> {
		// wait for the network to be online and have all its protocols installed
		// and addresses resolved
		self.handle.local.online().await;

		// watch for local address changes, this will also trigger the first
		// local peer entry update
		let mut addr_change = self.handle.local.endpoint().watch_addr().stream();

		self
			.handle
			.local
			.endpoint()
			.discovery()
			.add(self.provider.clone());

		loop {
			tokio::select! {
				// Network graceful termination signal
				() = self.handle.local.termination().cancelled() => {
					tracing::trace!(
						peer_id = %self.handle.local.id(),
						network = %self.handle.local.network_id(),
						"discovery protocol terminating"
					);
					return Ok(());
				}

				// Announcement protocol events
				Some(event) = self.announce.events().recv() => {
					self.on_announce_event(event);
				}

				// Catalog sync protocol events
				Some(event) = self.sync.events().recv() => {
					tracing::trace!(event = %Short(&event), "catalog sync event");
					self.on_catalog_sync_event(&event);
					self.events.send(event).ok();
				}

				// External commands from the handle
				Some(command) = self.commands.recv() => {
					self.on_external_command(command).await?;
				}

				// Completed catalog sync tasks
				Some(result) = self.syncs.join_next() => {
					if let Err(e) = result {
						tracing::warn!(
							error = %e,
							network = %self.handle.local.network_id(),
							"Discovery Catalog Sync task failed"
						);
					}
				}

				// observe local transport-level address changes
				Some(addr) = addr_change.next() => {
					tracing::trace!(addr = ?addr, "Local node address updated");
					self.handle.update_local_entry(move |entry| {
						entry.update_address(addr.clone())
							.expect("peer id changed for local node.")
					});
				}
			}
		}
	}

	/// Handle commands sent from the discovery handle
	async fn on_external_command(
		&mut self,
		command: WorkerCommand,
	) -> Result<(), Error> {
		match command {
			WorkerCommand::DialPeers(peers, resp) => {
				self.announce.dial(peers).await;
				let _ = resp.send(());
			}
			WorkerCommand::AcceptCatalogSync(connection) => {
				let peer_id = connection.remote_id();
				if let Err(e) = self.sync.protocol().accept(connection).await {
					tracing::warn!(
						error = %e,
						peer_id = %peer_id,
						network = %self.handle.local.network_id(),
						"Failed to accept catalog sync connection"
					);
				}
			}
			WorkerCommand::AcceptAnnounce(connection) => {
				let peer_id = connection.remote_id();
				if let Err(e) = self.announce.protocol().accept(connection).await {
					tracing::warn!(
						error = %e,
						peer_id = %peer_id,
						network = %self.handle.local.network_id(),
						"Failed to accept announce connection"
					);
				}
			}
			WorkerCommand::SyncWith(peer_id, done) => {
				self.on_manual_sync_request(peer_id, done);
			}
		}
		Ok(())
	}

	/// Handles events emitted by the announce protocol when receiving peer
	/// entries over gossip
	fn on_announce_event(&mut self, event: announce::Event) {
		match event {
			announce::Event::PeerEntryReceived(signed_peer_entry) => {
				// Update the catalog with the received peer entry
				self.handle.catalog.send_if_modified(|catalog| {
					let incoming_version = signed_peer_entry.update_version();
					match catalog.upsert_signed(signed_peer_entry) {
						UpsertResult::New(signed_peer_entry) => {
							tracing::debug!(
								info = %Short(signed_peer_entry),
								network = %self.handle.local.network_id(),
								"new peer discovered"
							);

							// Publish the new peer entry to the static provider
							let endpoint_info = signed_peer_entry.address().clone().into();
							self
								.handle
								.local
								.endpoint()
								.discovery()
								.publish(&endpoint_info);

							// Trigger a full catalog sync with the newly discovered peer
							self.syncs.spawn(
								self.sync.sync_with(signed_peer_entry.address().clone()),
							);

							self
								.events
								.send(Event::PeerDiscovered(signed_peer_entry.into()))
								.ok();

							true
						}
						UpsertResult::Updated(signed_peer_entry) => {
							tracing::debug!(
								info = %Short(signed_peer_entry),
								network = %self.handle.local.network_id(),
								"peer info updated"
							);

							// Publish the new peer entry to the static provider
							let endpoint_info = signed_peer_entry.address().clone().into();
							self
								.handle
								.local
								.endpoint()
								.discovery()
								.publish(&endpoint_info);

							self
								.events
								.send(Event::PeerUpdated(signed_peer_entry.clone().into()))
								.ok();
							true
						}
						UpsertResult::Rejected(signed_peer_entry) => {
							tracing::trace!(
								current = %Short(signed_peer_entry),
								network = %self.handle.local.network_id(),
								incoming_version = %incoming_version,
								current_version = %signed_peer_entry.update_version(),
								"stale peer update rejected"
							);
							false
						}
						UpsertResult::DifferentNetwork(peer_network) => {
							tracing::trace!(
								peer_network = %Short(peer_network),
								this_network = %Short(self.handle.local.network_id()),
								"peer entry from different network rejected"
							);
							false
						}
					}
				});
			}
		}
	}

	fn on_catalog_sync_event(&mut self, event: &Event) {
		match event {
			Event::PeerDiscovered(entry) | Event::PeerUpdated(entry) => {
				self.announce.observe(entry);
				self.register_peers_addresses(once(entry.address()));
			}
			Event::PeerDeparted(_) => {}
		}
	}

	/// Invoked with an manual catalog sync request from the handle is initiated
	/// through the public api.
	fn on_manual_sync_request(
		&mut self,
		peer: EndpointAddr,
		done: oneshot::Sender<Result<(), Error>>,
	) {
		// Register the peer addresses with the static addressing provider
		self.register_peers_addresses(once(&peer));

		let sync_fut = self.sync.sync_with(peer);
		self.syncs.spawn(async move {
			match sync_fut.await {
				Ok(()) => {
					let _ = done.send(Ok(()));
					Ok(())
				}
				Err(e) => {
					let wrapped = io::Error::other(e.to_string());
					let _ = done.send(Err(e));
					Err(Error::Other(wrapped.into()))
				}
			}
		});
	}

	/// Registers the addresses of a peer with the static addressing provider
	fn register_peers_addresses<'a>(
		&self,
		addr: impl Iterator<Item = &'a EndpointAddr>,
	) {
		for addr in addr {
			let endpoint_info = addr.clone().into();
			self
				.handle
				.local
				.endpoint()
				.discovery()
				.publish(&endpoint_info);
		}
	}
}

pub(super) enum WorkerCommand {
	/// Dial a peer with the given `EndpointAddr`s
	DialPeers(Vec<EndpointAddr>, oneshot::Sender<()>),

	/// Accept an incoming `CatalogSync` protocol connection from a remote peer
	AcceptCatalogSync(Connection),

	/// Accept an incoming `Announce` protocol connection from a remote peer
	AcceptAnnounce(Connection),

	/// Initiates a catalog sync with the given `PeerId`
	SyncWith(EndpointAddr, oneshot::Sender<Result<(), Error>>),
}
