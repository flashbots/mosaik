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
		network::{LocalNode, PeerId},
		primitives::IntoIterOrSingle,
	},
	core::{
		pin::Pin,
		task::{Context, Poll},
	},
	futures::{FutureExt, StreamExt},
	iroh::{Watcher, endpoint::Connection, protocol::DynProtocolHandler},
	tokio::{
		sync::{
			broadcast,
			mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
			oneshot,
			watch,
		},
		task::{JoinError, JoinHandle, JoinSet},
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
	pub catalog: watch::Receiver<Catalog>,
	pub events: broadcast::Receiver<Event>,
	pub commands: UnboundedSender<WorkerCommand>,
	pub task: JoinHandle<Result<(), Error>>,
}

impl Handle {
	/// Sends a command to dial a peer with the given `PeerId`
	pub async fn dial<V>(&self, peers: impl IntoIterOrSingle<PeerId, V>) {
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

	/// Sends a command to update the local peer entry using the provided update
	/// function.
	pub fn update_local_peer_entry(
		&self,
		update: impl FnOnce(PeerEntry) -> PeerEntry + Send + 'static,
	) {
		self
			.commands
			.send(WorkerCommand::UpdateLocalPeerEntry(Box::new(update)))
			.expect("Discovery worker loop is down");
	}
}

impl Future for Handle {
	type Output = Result<Result<(), Error>, JoinError>;

	fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
		self.get_mut().task.poll_unpin(cx)
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
	local: LocalNode,
	sync: CatalogSync,
	announce: Announce,
	catalog: watch::Sender<Catalog>,
	events: broadcast::Sender<Event>,
	syncs: JoinSet<Result<(), Error>>,
	commands: UnboundedReceiver<WorkerCommand>,
}

impl WorkerLoop {
	/// Constructs a new discovery worker loop and spawns it as a background task.
	/// Returns a handle to interact with the worker loop. This is called from
	/// the `Discovery::new` method.
	#[expect(clippy::needless_pass_by_value)]
	pub(super) fn spawn(local: LocalNode, config: Config) -> Handle {
		let (catalog_tx, catalog_rx) =
			watch::channel(Catalog::new(&local, &config));
		let (commands_tx, commands_rx) = unbounded_channel();
		let (events_tx, events_rx) = broadcast::channel(config.events_backlog);

		let sync = CatalogSync::new(local.clone(), catalog_tx.clone());
		let announce = Announce::new(local.clone(), &config, catalog_rx.clone());

		let worker = Self {
			local,
			sync,
			announce,
			catalog: catalog_tx,
			events: events_tx,
			syncs: JoinSet::new(),
			commands: commands_rx,
		};

		// spawn the worker loop
		let task = tokio::spawn(async move {
			let local = worker.local.clone();
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
		Handle {
			catalog: catalog_rx,
			events: events_rx,
			commands: commands_tx,
			task,
		}
	}
}

impl WorkerLoop {
	async fn run(mut self) -> Result<(), Error> {
		// wait for the network to be online and have all its protocols installed
		// and addresses resolved
		self.local.online().await;

		// watch for local address changes, this will also trigger the first
		// local peer entry update
		let mut addr_change = self.local.endpoint().watch_addr().stream();

		loop {
			tokio::select! {
				// Network graceful termination signal
				() = self.local.termination().cancelled() => {
					tracing::info!("Discovery worker loop terminating");
					return Ok(());
				}

				// Announcement protocol events
				Some(event) = self.announce.events().recv() => {
					tracing::debug!(event = ?event, "Discovery announce event");
					self.on_announce_event(event);
				}

				// Catalog sync protocol events
				Some(event) = self.sync.events().recv() => {
					tracing::debug!(event = ?event, "Discovery catalog sync event");
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
							network = %self.local.network_id(),
							"Catalog sync task failed"
						);
					}
				}

				// observe local transport-level address changes
				Some(addr) = addr_change.next() => {
					tracing::debug!(addr = ?addr, "Local node address updated");
					self.update_local_peer_entry(|entry| {
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
				tracing::info!("Dialing peers {peers:?}");
				self.announce.dial(peers);
				let _ = resp.send(());
			}
			WorkerCommand::UpdateLocalPeerEntry(update_fn) => {
				self.update_local_peer_entry(update_fn);
			}
			WorkerCommand::AcceptCatalogSync(connection) => {
				let peer_id = connection.remote_id();
				if let Err(e) = self.sync.protocol().accept(connection).await {
					tracing::warn!(
						error = %e,
						peer_id = %peer_id,
						network = %self.local.network_id(),
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
						network = %self.local.network_id(),
						"Failed to accept announce connection"
					);
				}
			}
			WorkerCommand::InsertUnsignedPeer(entry) => {
				self.insert_unsigned_peer(entry);
			}
			WorkerCommand::RemoveUnsignedPeer(peer_id) => {
				self.remove_unsigned_peer(&peer_id);
			}
			WorkerCommand::ClearUnsignedPeers => {
				self.clear_unsigned_peers();
			}
		}
		Ok(())
	}

	/// Handles events emitted by the announce protocol when receiving peer
	/// entries over gossip
	fn on_announce_event(&mut self, event: announce::Event) {
		match event {
			announce::Event::PeerEntryReceived(signed_peer_entry) => {
				tracing::debug!(
					info = ?signed_peer_entry,
					network = %self.local.network_id(),
					"Received peer entry via discovery announcement"
				);

				// Update the catalog with the received peer entry
				self.catalog.send_if_modified(|catalog| {
					match catalog.upsert_signed(signed_peer_entry) {
						UpsertResult::New(signed_peer_entry) => {
							tracing::info!(
								info = ?signed_peer_entry,
								network = %self.local.network_id(),
								"New peer discovered"
							);

							// Trigger a full catalog sync with the newly discovered peer
							self
								.syncs
								.spawn(self.sync.sync_with(*signed_peer_entry.id()));

							self
								.events
								.send(Event::PeerDiscovered(signed_peer_entry.clone().into()))
								.ok();
							true
						}
						UpsertResult::Updated(signed_peer_entry) => {
							tracing::debug!(
								info = ?signed_peer_entry,
								network = %self.local.network_id(),
								"Peer updated via discovery"
							);
							self
								.events
								.send(Event::PeerUpdated(signed_peer_entry.clone().into()))
								.ok();
							true
						}
						UpsertResult::Rejected(signed_peer_entry) => {
							tracing::debug!(
								current = ?signed_peer_entry,
								network = %self.local.network_id(),
								"Stale peer update rejected via discovery"
							);
							false
						}
					}
				});
			}
		}
	}

	/// Updates the local peer entry using the provided update function.
	///
	/// The effects of this update are:
	/// - The local peer entry in the catalog is updated.
	/// - The catalog watch channel is notified of the change.
	/// - The announcement protocol will pick up the change and broadcast the
	///   updated entry immediately to all peers.
	fn update_local_peer_entry(
		&mut self,
		update: impl FnOnce(PeerEntry) -> PeerEntry,
	) {
		self.catalog.send_modify(|catalog| {
			let local_entry = catalog.local().clone();
			let updated_entry = update(local_entry.into());
			let signed_updated_entry = updated_entry
				.sign(self.local.secret_key())
				.expect("signing updated local peer entry failed.");

			tracing::info!(
				info = ?signed_updated_entry,
				network = %self.local.network_id(),
				"Updating local peer entry in catalog"
			);

			assert!(
				catalog.upsert_signed(signed_updated_entry).is_ok(),
				"local peer info versioning error. this is a bug."
			);
		});
	}

	/// Inserts an unsigned peer entry into the catalog that is only used locally
	/// by this node and is not synced to other peers.
	fn insert_unsigned_peer(&mut self, entry: PeerEntry) {
		self
			.catalog
			.send_if_modified(|catalog| catalog.insert_unsigned(entry));
	}

	/// Removes an unsigned peer entry from the catalog by its [`PeerId`].
	fn remove_unsigned_peer(&mut self, peer_id: &PeerId) {
		self
			.catalog
			.send_if_modified(|catalog| catalog.remove_unsigned(peer_id).is_some());
	}

	/// Clears all unsigned peer entries from the catalog.
	fn clear_unsigned_peers(&mut self) {
		self
			.catalog
			.send_if_modified(|catalog| catalog.clear_unsigned());
	}
}

pub(super) enum WorkerCommand {
	/// Dial a peer with the given `PeerId`s
	DialPeers(Vec<PeerId>, oneshot::Sender<()>),

	/// Update the local peer entry using the provided `PeerEntry` update function
	UpdateLocalPeerEntry(Box<dyn FnOnce(PeerEntry) -> PeerEntry + Send>),

	/// Accept an incoming `CatalogSync` protocol connection from a remote peer
	AcceptCatalogSync(Connection),

	/// Accept an incoming `Announce` protocol connection from a remote peer
	AcceptAnnounce(Connection),

	/// Adds a new unsigned peer entry to the catalog that is used locally by this
	/// node but is not synced to other peers.
	InsertUnsignedPeer(PeerEntry),

	/// Removes an unsigned peer entry from the catalog by its [`PeerId`].
	RemoveUnsignedPeer(PeerId),

	/// Clears all unsigned peer entries from the catalog.
	ClearUnsignedPeers,
}
