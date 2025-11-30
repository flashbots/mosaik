use {
	super::{
		Catalog,
		Config,
		Error,
		Event,
		announce::Announce,
		sync::CatalogSync,
	},
	crate::{LocalNode, PeerEntry, PeerId},
	futures::StreamExt,
	iroh::{Watcher, endpoint::Connection, protocol::DynProtocolHandler},
	tokio::{
		sync::{
			broadcast,
			mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
			oneshot,
			watch,
		},
		task::JoinHandle,
	},
	tracing::info,
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
	pub(super) async fn dial(&self, peers: impl IntoIterator<Item = PeerId>) {
		let (tx, rx) = oneshot::channel();
		self
			.commands
			.send(WorkerCommand::DialPeers(peers.into_iter().collect(), tx))
			.ok();
		let _ = rx.await;
	}

	/// Sends a command to update the local peer entry using the provided update
	/// function.
	pub(super) async fn update_local_peer_entry(
		&self,
		update: impl FnOnce(PeerEntry) -> PeerEntry + Send + 'static,
	) {
		let (tx, rx) = oneshot::channel();
		self
			.commands
			.send(WorkerCommand::UpdateLocalPeerEntry(Box::new(update), tx))
			.ok();
		let _ = rx.await;
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
	commands: UnboundedReceiver<WorkerCommand>,
}

impl WorkerLoop {
	/// Constructs a new discovery worker loop and spawns it as a background task.
	/// Returns a handle to interact with the worker loop. This is called from
	/// the `Discovery::new` method.
	pub(super) fn spawn(local: LocalNode, config: Config) -> Handle {
		let (catalog_tx, catalog_rx) = watch::channel(Catalog::new(&local));
		let (commands_tx, commands_rx) = unbounded_channel();
		let (events_tx, events_rx) = broadcast::channel(config.events_backlog);

		let sync = CatalogSync::new(local.clone(), catalog_rx.clone());
		let announce = Announce::new(local.clone(), &config, catalog_rx.clone());

		let worker = Self {
			local,
			sync,
			announce,
			catalog: catalog_tx,
			events: events_tx,
			commands: commands_rx,
		};

		// spawn the worker loop
		let task = tokio::spawn(worker.run());

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
				_ = self.local.termination().cancelled() => {
					info!("Discovery worker loop terminating");
					return Ok(());
				}

				// Announcement protocol events
				Some(event) = self.announce.events().recv() => {
					tracing::info!("Discovery announce event: {event:?}");
					self.events.send(Event::Announcement(event)).ok();
				}

				// Catalog sync protocol events
				Some(event) = self.sync.events().recv() => {
					tracing::info!("Discovery catalog sync event: {event:?}");
					self.events.send(Event::CatalogSync(event)).ok();
				}

				// External commands from the handle
				Some(command) = self.commands.recv() => {
					self.on_external_command(command).await?;
				}

				// observe local transport-level address changes
				Some(addr) = addr_change.next() => {
					tracing::info!("Local node address updated to {addr:?}");
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
			WorkerCommand::UpdateLocalPeerEntry(update_fn, resp) => {
				self.update_local_peer_entry(update_fn);
				let _ = resp.send(());
			}
			WorkerCommand::AcceptCatalogSync(_) => {
				tracing::info!("Accepting catalog sync connection");
			}
			WorkerCommand::AcceptAnnounce(connection) => {
				tracing::info!(
					"Accepting announce connection from {}",
					connection.remote_id()
				);

				if let Err(e) = self.announce.protocol().accept(connection).await {
					tracing::warn!("{e}");
				}
			}
		}
		Ok(())
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
}

pub(super) enum WorkerCommand {
	/// Dial a peer with the given PeerId
	DialPeers(Vec<crate::PeerId>, oneshot::Sender<()>),

	/// Update the local peer entry using the provided PeerEntry
	UpdateLocalPeerEntry(
		Box<dyn FnOnce(PeerEntry) -> PeerEntry + Send>,
		oneshot::Sender<()>,
	),

	/// Accept an incoming CatalogSync protocol connection from a remote peer
	AcceptCatalogSync(Connection),

	/// Accept an incoming Announce protocol connection from a remote peer
	AcceptAnnounce(Connection),
}
