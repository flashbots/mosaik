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
		PeerId,
		discovery::{PeerEntryVersion, SignedPeerEntry, bootstrap::DhtBootstrap},
		network::LocalNode,
		primitives::{IntoIterOrSingle, Pretty, Short},
	},
	chrono::Utc,
	core::time::Duration,
	futures::{StreamExt, TryFutureExt},
	iroh::{
		EndpointAddr,
		Watcher,
		endpoint::Connection,
		protocol::DynProtocolHandler,
	},
	rand::Rng,
	std::{io, sync::Arc},
	tokio::{
		sync::{
			broadcast,
			mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
			oneshot,
			watch,
		},
		task::JoinSet,
		time::interval,
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
		self.catalog.send_if_modified(|catalog| {
			let local_entry = catalog.local().clone();
			let prev_version = local_entry.update_version();
			let updated_entry = update(local_entry.into());
			if updated_entry.update_version() > prev_version {
				let signed_updated_entry = updated_entry
					.sign(self.local.secret_key())
					.expect("signing updated local peer entry failed.");

				tracing::trace!(
					peer_info = %Short(&signed_updated_entry),
					network = %self.local.network_id(),
					"updated local",
				);

				assert!(
					catalog.upsert_signed(signed_updated_entry).is_ok(),
					"local peer info versioning error. this is a bug."
				);
				true
			} else {
				false
			}
		});
	}

	/// Performs a full catalog synchronization with the specified peer.
	///
	/// This async method resolves when the sync is complete or fails.
	pub fn sync_with(
		&self,
		peer_addr: impl Into<EndpointAddr>,
	) -> impl Future<Output = Result<(), Error>> + Send + Sync + 'static {
		let peer_addr = peer_addr.into();
		let commands_tx = self.commands.clone();

		async move {
			let (tx, rx) = oneshot::channel();
			commands_tx
				.send(WorkerCommand::SyncWith(peer_addr, tx))
				.map_err(|_| Error::Cancelled)?;
			rx.await.map_err(|_| Error::Cancelled)?
		}
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
	/// Discovery configuration
	config: Arc<Config>,

	/// Allows the public API to interact with the internal worker loop.
	handle: Arc<Handle>,

	/// Full catalog sync protocol handler.
	sync: CatalogSync,

	/// Peer gossip announcement protocol handler.
	announce: Announce,

	/// Automatic DHT bootstrap system
	bootstrap: DhtBootstrap,

	/// Public API discovery events.
	events: broadcast::Sender<Event>,

	/// Ongoing catalog sync tasks.
	syncs: JoinSet<Result<PeerId, (Error, PeerId)>>,

	/// Incoming commands from the public API.
	commands: UnboundedReceiver<WorkerCommand>,

	/// Interval timer for periodic announcements of the local peer entry.
	/// On each tick, the local peer entry is re-broadcasted with an updated
	/// version to ensure peers remain aware of its presence.
	announce_interval: tokio::time::Interval,

	/// Interval timer for periodic purging of stale peer entries from the
	/// discovery catalog.
	purge_interval: tokio::time::Interval,
}

impl WorkerLoop {
	/// Constructs a new discovery worker loop and spawns it as a background task.
	/// Returns a handle to interact with the worker loop. This is called from
	/// the `Discovery::new` method.
	pub(super) fn spawn(local: LocalNode, config: Config) -> Arc<Handle> {
		let config = Arc::new(config);
		let catalog = watch::Sender::new(Catalog::new(&local, &config));
		let (commands_tx, commands_rx) = unbounded_channel();
		let (events_tx, events_rx) = broadcast::channel(config.events_backlog);

		let sync = CatalogSync::new(local.clone(), catalog.clone());
		let announce = Announce::new(local.clone(), &config, catalog.subscribe());
		let announce_interval = interval(config.announce_interval);
		let purge_interval = interval(config.purge_after);

		let handle = Arc::new(Handle {
			local,
			catalog,
			events: events_rx,
			commands: commands_tx,
		});

		let bootstrap = DhtBootstrap::new(
			Arc::clone(&handle),
			&config,
			handle.catalog.subscribe(),
		);

		let worker = Self {
			config,
			handle: Arc::clone(&handle),
			sync,
			announce,
			bootstrap,
			events: events_tx,
			syncs: JoinSet::new(),
			commands: commands_rx,
			announce_interval,
			purge_interval,
		};

		// spawn the worker loop
		tokio::spawn(async move {
			let local = worker.handle.local.clone();
			let result = worker.run().await;

			if let Err(ref e) = result {
				tracing::error!(
					error = %e,
					network = %local.network_id(),
					"Discovery subsystem terminated"
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

				// Automatic DHT bootstrap peer discovered
				Some(peer_id) = self.bootstrap.events().recv() => {
					self.on_dht_discovery(peer_id).await;
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

				// Drive catalog sync tasks
				Some(Ok(result)) = self.syncs.join_next() => {
					self.on_catalog_sync_complete(result);
				}

				// observe local transport-level address changes
				Some(addr) = addr_change.next() => {
					tracing::trace!(
						address = %Pretty(&addr),
						network = %self.handle.local.network_id(),
						"updated local"
					);

					self.handle.update_local_entry(move |entry| {
						entry.update_address(addr)
							.expect("peer id changed for local node.")
					});
				}

				// Periodic announcement tick
				_ = self.announce_interval.tick() => {
					self.on_periodic_announce_tick();
				}

				// Periodic catalog purge tick
				_ = self.purge_interval.tick() => {
					self.on_periodic_catalog_purge_tick();
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
					tracing::trace!(
						error = %e,
						peer_id = %Short(&peer_id),
						network = %self.handle.local.network_id(),
						"failed to accept catalog sync connection"
					);
				}
			}
			WorkerCommand::AcceptAnnounce(connection) => {
				let peer_id = connection.remote_id();
				if let Err(e) = self.announce.protocol().accept(connection).await {
					tracing::trace!(
						error = %e,
						peer_id = %peer_id,
						network = %self.handle.local.network_id(),
						"Failed to accept announce connection"
					);
				}
			}
			WorkerCommand::SyncWith(peer_id, done) => {
				self.on_explicit_sync_request(peer_id, done);
			}
		}
		Ok(())
	}

	/// Handles events emitted by the announce protocol when receiving peer
	/// entries over gossip
	fn on_announce_event(&mut self, event: announce::Event) {
		match event {
			announce::Event::PeerEntryReceived(signed_peer_entry) => {
				self.bootstrap.note_healthy(*signed_peer_entry.id());
				self.on_peer_entry_received(signed_peer_entry);
			}

			announce::Event::PeerDeparted(peer_id, entry_version) => {
				self.bootstrap.note_unhealthy(peer_id);
				self.on_peer_departed(peer_id, entry_version);
			}
		}
	}

	/// Handles peers discovered via the Mainline DHT bootstrap mechanism.
	async fn on_dht_discovery(&self, peer_id: PeerId) {
		if self.handle.catalog.borrow().get(&peer_id).is_none() {
			tracing::trace!(
				peer_id = %Short(&peer_id),
				network = %self.handle.local.network_id(),
				"peer discovered via DHT auto bootstrap"
			);

			self.announce.dial(peer_id).await;
		}
	}

	/// Invoked when a new signed peer entry is received from the announce
	/// protocol over gossip. Attempts to upsert the entry into the local catalog
	/// and triggers appropriate actions based on whether the entry is new,
	/// updated, or rejected.
	fn on_peer_entry_received(&mut self, peer_entry: SignedPeerEntry) {
		let modified = self.handle.catalog.send_if_modified(|catalog| {
			match catalog.upsert_signed(peer_entry) {
				UpsertResult::New(peer_entry) => {
					tracing::debug!(
						peer = %Short(peer_entry),
						network = %self.handle.local.network_id(),
						"discovered new"
					);

					// Publish the new peer entry to the static provider
					self.handle.local.observe(peer_entry.address());

					// Trigger a full catalog sync with the newly discovered peer
					let peer_id = *peer_entry.id();
					self.syncs.spawn(
						self
							.sync
							.sync_with(peer_entry.address().clone())
							.map_ok(move |()| peer_id)
							.map_err(move |e| (e, peer_id)),
					);

					self
						.events
						.send(Event::PeerDiscovered(peer_entry.into()))
						.ok();

					true
				}
				UpsertResult::Updated(peer_entry) => {
					tracing::trace!(
						peer_info = %Short(peer_entry),
						network = %self.handle.local.network_id(),
						"updated"
					);

					// Publish the new peer entry to the static provider
					self.handle.local.observe(peer_entry.address());
					self.events.send(Event::PeerUpdated(peer_entry.into())).ok();
					true
				}
				UpsertResult::Outdated(peer_entry) => {
					tracing::trace!(
						peer_info = %Short(peer_entry.as_ref()),
						network = %self.handle.local.network_id(),
						"rejected outdated"
					);
					false
				}
				UpsertResult::Rejected { rejected, existing } => {
					if rejected.update_version() < existing.update_version() {
						tracing::trace!(
							update = %Short(rejected.as_ref()),
							existing = %Short(existing),
							network = %self.handle.local.network_id(),
							"rejected stale peer"
						);
					}
					false
				}
				UpsertResult::DifferentNetwork(peer_network) => {
					tracing::trace!(
						peer_network = %Short(peer_network),
						this_network = %Short(self.handle.local.network_id()),
						"rejected peer info update from different network"
					);
					false
				}
			}
		});

		if modified {
			let purge_in = self.next_purge_deadline();
			self.purge_interval.reset_after(purge_in);
		}
	}

	/// Handles a graceful peer departure event received from the announce
	/// protocol. Marks the peer as departed in the local catalog if the last
	/// known version is equal or newer than the current entry and the timestamp
	/// of the departure message is recent enough.
	fn on_peer_departed(&self, peer_id: PeerId, entry_version: PeerEntryVersion) {
		let Some(last_known_version) = self
			.handle
			.catalog
			.borrow()
			.get_signed(&peer_id)
			.map(|e| e.update_version())
		else {
			// unknown peer, nothing to do
			return;
		};

		if entry_version < last_known_version {
			// stale departure event, ignore
			return;
		}

		let modified = self
			.handle
			.catalog
			.send_if_modified(|catalog| catalog.remove_signed(&peer_id).is_some());

		if modified {
			tracing::trace!(
				peer = %Short(&peer_id),
				network = %self.handle.local.network_id(),
				"gracefully departed"
			);

			self.events.send(Event::PeerDeparted(peer_id)).ok();
		}
	}

	fn on_catalog_sync_event(&mut self, event: &Event) {
		match event {
			Event::PeerDiscovered(entry) | Event::PeerUpdated(entry) => {
				self.announce.observe(entry);
				self.handle.local.observe(entry.address());
			}
			Event::PeerDeparted(_) => {}
		}

		let purge_in = self.next_purge_deadline();
		self.purge_interval.reset_after(purge_in);
	}

	/// Invoked with an manual catalog sync request from the handle is initiated
	/// through the public api.
	fn on_explicit_sync_request(
		&mut self,
		peer: EndpointAddr,
		done: oneshot::Sender<Result<(), Error>>,
	) {
		let peer_id = peer.id;
		self.handle.local.observe(&peer);
		let sync_fut = self.sync.sync_with(peer);
		self.syncs.spawn(async move {
			match sync_fut.await {
				Ok(()) => {
					let _ = done.send(Ok(()));
					Ok(peer_id)
				}
				Err(e) => {
					let wrapped = io::Error::other(e.to_string());
					let _ = done.send(Err(e));
					Err((Error::Other(wrapped.into()), peer_id))
				}
			}
		});
	}

	fn on_catalog_sync_complete(&self, result: Result<PeerId, (Error, PeerId)>) {
		match result {
			Ok(peer_id) => {
				self.bootstrap.note_healthy(peer_id);
			}
			Err((e, peer_id)) => {
				tracing::trace!(
					error = %e,
					peer_id = %Short(&peer_id),
					network = %self.handle.local.network_id(),
					"catalog sync failed"
				);
				self.bootstrap.note_unhealthy(peer_id);
			}
		}
	}

	/// Periodically increment the local peer entry version and re-announce it
	/// to ensure peers are aware of our continued presence on the network.
	fn on_periodic_announce_tick(&mut self) {
		// Calculate next announce delay with jitter
		let base = self.config.announce_interval;
		let max_jitter = base.mul_f32(self.config.announce_jitter);
		let jitter = rand::rng().random_range(Duration::ZERO..=max_jitter * 2);
		let next_announce = (base + jitter).saturating_sub(max_jitter);

		// Schedule the next announce with jitter
		self.announce_interval.reset_after(next_announce);

		// update local peer entry by incrementing its version to the current time
		// to trigger re-announcement
		self
			.handle
			.update_local_entry(|entry| entry.increment_version());
	}

	/// Periodically purge stale peer entries from the catalog.
	/// Configured via the `purge_interval` config parameter.
	fn on_periodic_catalog_purge_tick(&mut self) {
		let mut purged = vec![];
		self.handle.catalog.send_if_modified(|catalog| {
			purged = catalog.purge_stale_entries().collect();
			!purged.is_empty()
		});

		if purged.is_empty() {
			return;
		}

		for peer in &purged {
			self.events.send(Event::PeerDeparted(*peer.id())).ok();
		}

		tracing::debug!(
			peers = %Short::iter(purged.iter().map(|p| p.id())),
			network = %self.handle.local.network_id(),
			"purged {} stale peers", purged.len()
		);

		let next_purge_in = self.next_purge_deadline();
		self.purge_interval.reset_after(next_purge_in);
	}

	/// Calculates the next deadline for purging stale entries from the catalog by
	/// finding the soonest expiration time among all entries in the catalog.
	fn next_purge_deadline(&self) -> Duration {
		let now = Utc::now();
		let mut deadline = self.config.purge_after;
		let catalog = self.handle.catalog.borrow().clone();

		for peer in catalog.signed_peers() {
			let expires_at = peer.updated_at() + self.config.purge_after;
			let expires_in = expires_at
				.signed_duration_since(now)
				.to_std()
				.unwrap_or_default();
			deadline = deadline.min(expires_in);

			if deadline.is_zero() {
				break;
			}
		}

		deadline
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
