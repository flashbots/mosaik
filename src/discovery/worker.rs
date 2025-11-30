use {
	super::{Catalog, CatalogSync, Config, Discovery, Error, Event},
	crate::{LocalNode, PeerEntry},
	bincode::{config::standard, serde::encode_to_vec},
	futures::StreamExt,
	im::ordmap,
	iroh::endpoint::Connection,
	iroh_gossip::{
		Gossip,
		TopicId,
		api::{ApiError as GossipError, Event as GossipEvent, GossipTopic},
	},
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
	pub gossip: Gossip,
	pub sync: CatalogSync,
	pub task: JoinHandle<Result<(), Error>>,
}

impl Handle {
	/// Sends a command to dial a peer with the given `PeerId`
	pub(super) async fn dial(&self, peer: crate::PeerId) {
		let (tx, rx) = oneshot::channel();
		self.commands.send(WorkerCommand::DialPeer(peer, tx)).ok();
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
	config: Config,
	local: LocalNode,
	gossip: Gossip,
	sync: CatalogSync,
	catalog: watch::Sender<Catalog>,
	events: broadcast::Sender<Event>,
	commands: UnboundedReceiver<WorkerCommand>,
}

impl WorkerLoop {
	/// Constructs a new discovery worker loop and spawns it as a background task.
	/// Returns a handle to interact with the worker loop. This is called from
	/// the `Discovery::new` method.
	pub(super) fn spawn(local: LocalNode, config: Config) -> Handle {
		let (events_tx, events_rx) = broadcast::channel(config.events_backlog);
		let (catalog_tx, catalog_rx) = watch::channel(Catalog::new());
		let (commands_tx, commands_rx) = unbounded_channel();

		let gossip = Gossip::builder()
			.alpn(Discovery::ALPN_GOSSIP)
			.spawn(local.endpoint().clone());

		let sync = CatalogSync::new(local.clone(), catalog_tx.subscribe());

		let worker = Self {
			config,
			local,
			gossip: gossip.clone(),
			sync: sync.clone(),
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
			gossip,
			sync,
			task,
		}
	}
}

impl WorkerLoop {
	async fn run(mut self) -> Result<(), Error> {
		// wait for the network to be online and have all its protocols installed
		// and addresses resolved
		self.local.online().await;

		// join the gossip topic
		let mut topic = self.join_gossip_topic().await?;

		// initialize the local peer entry in the catalog
		let initial_local_entry =
			PeerEntry::new(self.local.addr()).add_tags(self.config.tags.clone());

		self
			.update_local_peer_entry(|_| initial_local_entry, &mut topic)
			.await?;

		loop {
			tokio::select! {
				// Gossip events
				event = topic.next() => {
					match event {
						None | Some(Err(GossipError::Closed { .. })) => {
							// topic connection dropped, re-join
							tracing::warn!(
								network = %self.local.network_id(),
								"Gossip topic connection closed, re-joining"
							);

							topic = self.join_gossip_topic().await?;
						},
						Some(Err(e)) => {
							tracing::warn!(
								network = %self.local.network_id(),
								"Gossip topic error: {e}"
							);
						},
						Some(Ok(event)) => {
							self.on_gossip_event(event, &mut topic).await?;
						}
					}
				}

				// External commands from the handle
				Some(command) = self.commands.recv() => {
					tracing::info!("Discovery worker received command");
					self.on_external_command(command, &mut topic).await?;
				}
			}
		}
	}

	/// Initializes the gossip topic for discovery.
	///
	/// This joins an iroh-gossip topic based on the network ID.
	async fn join_gossip_topic(&self) -> Result<GossipTopic, Error> {
		let topic_id: TopicId = self.local.network_id().into();
		let bootstrap = self.config.bootstrap_peers.clone();
		let topic = self.gossip.subscribe(topic_id, bootstrap).await?;
		Ok(topic)
	}

	/// Handle gossip-level events
	async fn on_gossip_event(
		&mut self,
		event: GossipEvent,
		topic: &mut GossipTopic,
	) -> Result<(), Error> {
		tracing::info!("Received gossip event: {event:?}");
		match event {
			GossipEvent::NeighborUp(_) => {
				self.broadcast_self_info(topic).await?;
			}
			e => info!(
				event = ?e,
				network = %self.local.network_id(),
				"Gossip Event"
			),
		};
		Ok(())
	}

	/// Handle commands sent from the discovery handle
	async fn on_external_command(
		&mut self,
		command: WorkerCommand,
		topic: &mut GossipTopic,
	) -> Result<(), Error> {
		match command {
			WorkerCommand::DialPeer(peer_id, resp) => {
				tracing::info!("Dialing peer {peer_id}");
				let _ = resp.send(());
			}
			WorkerCommand::UpdateLocalPeerEntry(update_fn, resp) => {
				self.update_local_peer_entry(update_fn, topic).await?;
				let _ = resp.send(());
			}
			WorkerCommand::AcceptCatalogSync(_) => {
				tracing::info!("Accepting catalog sync connection");
			}
		}
		Ok(())
	}

	/// Updates the local peer entry using the provided update function.
	/// And updates the catalog watch with the latest version of the catalog.
	async fn update_local_peer_entry(
		&mut self,
		update: impl FnOnce(PeerEntry) -> PeerEntry,
		topic: &mut GossipTopic,
	) -> Result<(), Error> {
		let mut catalog = self.catalog.borrow().clone();
		let updated = match catalog.signed.entry(self.local.id()) {
			ordmap::Entry::Occupied(mut entry) => {
				let current: PeerEntry = (**entry.get()).clone();
				let current_version = current.version();
				let next = update(current);
				if next.version() > current_version {
					entry.insert(next.sign(self.local.secret_key())?);
					true
				} else {
					false
				}
			}
			ordmap::Entry::Vacant(entry) => {
				let new = PeerEntry::new(self.local.endpoint().addr().clone());
				let next = update(new);
				entry.insert(next.sign(self.local.secret_key())?);
				true
			}
		};

		if updated {
			// The new local entry has been updated, broadcast it to the network
			// and update the catalog watch
			self.broadcast_self_info(topic).await?;
			self
				.catalog
				.send(catalog)
				.expect("Catalog watch receiver dropped");
		}

		Ok(())
	}

	/// Broadcasts the local peer entry to the gossip topic.
	async fn broadcast_self_info(
		&self,
		topic: &mut GossipTopic,
	) -> Result<(), Error> {
		if !topic.is_joined() {
			return Ok(());
		}

		let catalog = self.catalog.borrow().clone();
		if let Some(my_info) = catalog.signed.get(&self.local.id()) {
			topic
				.broadcast(
					encode_to_vec(my_info, standard())
						.expect("Encoding failed")
						.into(),
				)
				.await?;

			info!(
				info = ?my_info,
				network = %self.local.network_id(),
				"Broadcasted local peer entry"
			);
		}
		Ok(())
	}
}

pub(super) enum WorkerCommand {
	/// Dial a peer with the given PeerId
	DialPeer(crate::PeerId, oneshot::Sender<()>),

	/// Update the local peer entry using the provided PeerEntry
	UpdateLocalPeerEntry(
		Box<dyn FnOnce(PeerEntry) -> PeerEntry + Send>,
		oneshot::Sender<()>,
	),

	/// Accept an incoming CatalogSync connection from a remote peer
	AcceptCatalogSync(Connection),
}
