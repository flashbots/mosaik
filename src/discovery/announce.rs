use {
	super::{Catalog, Config, Error, PeerEntryVersion, SignedPeerEntry},
	crate::{
		discovery::PeerEntry,
		network::{LocalNode, PeerId, link::Protocol},
		primitives::{Pretty, Short, UnboundedChannel},
	},
	bincode::{
		config::standard,
		serde::{decode_from_std_read, encode_to_vec},
	},
	bytes::Buf,
	core::sync::atomic::{AtomicUsize, Ordering},
	futures::StreamExt,
	iroh::protocol::ProtocolHandler,
	iroh_gossip::{
		Gossip,
		api::{
			ApiError as GossipError,
			Event as GossipEvent,
			GossipReceiver,
			GossipSender,
			GossipTopic,
		},
	},
	serde::{Deserialize, Serialize},
	std::sync::Arc,
	tokio::sync::{
		mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
		watch,
	},
	tokio_util::sync::CancellationToken,
	tracing::error,
};

#[derive(Debug, Clone)]
pub enum Event {
	/// A valid and signed peer entry has been updated.
	PeerEntryReceived(SignedPeerEntry),
}

/// The announcement protocol for broadcasting peer presence and metadata.
///
/// This protocol forms a gossip topic that is named after the network ID,
/// allowing peers to announce their presence and metadata changes in real time.
///
/// Notes:
///
/// - All announcements are signed by the peer's private key to ensure
///   authenticity.
/// - Announcements are broadcasted over the gossip topic to all subscribed
///   peers.
/// - The first announcement from a peer is broadcasted when they join the
///   gossip topic.
///
/// - Subsequent announcements are broadcasted whenever:
///   - the local peer entry is updated.
///   - a new direct gossip neighbor is detected.
///   - Periodically, to reaffirm presence at regular intervals with random
///     jitter.
///
/// - Peers listen for announcements from other peers and update their local
///   catalogs accordingly.
///
/// - Each announcement carries a version number to help peers determine the
///   most recent information, only updating their catalogs if the received
///   version is newer than the existing one.
///
/// - The catalog is the source of truth for the local peer entry; announcements
///   are generated based on the changes to the current state of the catalog.
pub(super) struct Announce {
	gossip: Gossip,
	events: UnboundedReceiver<Event>,
	dials: UnboundedSender<Vec<PeerId>>,
	neighbors_count: Arc<AtomicUsize>,
}

impl Protocol for Announce {
	/// ALPN identifier for the announcement protocol.
	///
	/// This overrides the default `iroh_gossip` ALPN to use a Mosaik-specific
	/// namespace.
	const ALPN: &'static [u8] = b"/mosaik/discovery/announce/1.0";
}

/// Public API for the announcement protocol
impl Announce {
	/// Initializes the announcement protocol with the given local node and
	/// configuration.
	///
	/// This sets up the gossip topic and prepares the protocol for operation.
	/// We need the catalog watch receiver to monitor local peer entry updates.
	pub(super) fn new(
		local: LocalNode,
		config: &Config,
		catalog: watch::Receiver<Catalog>,
	) -> Self {
		let gossip = Gossip::builder()
			.alpn(Self::ALPN)
			.spawn(local.endpoint().clone());

		let events = unbounded_channel();
		let dials = unbounded_channel();
		let cancel = local.termination().clone();
		let last_own_version = catalog.borrow().local().update_version();
		let neighbors_count = Arc::new(AtomicUsize::new(0));

		let driver = WorkerLoop {
			config: config.clone(),
			gossip: gossip.clone(),
			local: local.clone(),
			cancel: cancel.clone(),
			catalog,
			events: events.0,
			dials: dials.1,
			last_own_version,
			neighbors_count: Arc::clone(&neighbors_count),
			messages_in: UnboundedChannel::default(),
			messages_out: UnboundedChannel::default(),
		};

		// Spawn the worker loop task
		tokio::spawn(async move {
			if let Err(e) = driver.spawn().await {
				error!(
					error = %e,
					network_id = %local.network_id(),
					"Unrecoverable error in discovery protocol, terminating network"
				);

				// Trigger network termination
				cancel.cancel();
			}
		});

		Self {
			gossip,
			events: events.1,
			dials: dials.0,
			neighbors_count,
		}
	}

	/// Returns a mutable reference to the events receiver.
	///
	/// This is polled by the discovery worker to process incoming events from the
	/// announcement protocol.
	pub const fn events(&mut self) -> &mut UnboundedReceiver<Event> {
		&mut self.events
	}

	/// Dials the given peer address to initiate a discovery exchange.
	pub fn dial(&self, peers: Vec<PeerId>) {
		self.dials.send(peers).ok();
	}

	/// A hint to observe a peer.
	///
	/// This is useful when a peer does not have any connected gossip neighbors
	/// but it does a full catalog sync with another peer or learns in any other
	/// way about another peer and there are new potential peers to connect to.
	pub fn observe(&self, peer: &PeerEntry) {
		if self.neighbors_count.load(Ordering::SeqCst) == 0 {
			self.dial(vec![*peer.id()]);
		}
	}

	/// Returns the protocol listener instance responsible for accepting incoming
	/// connections for the announcement protocol.
	pub const fn protocol(&self) -> &impl ProtocolHandler {
		&self.gossip
	}
}

struct WorkerLoop {
	config: Config,
	gossip: Gossip,
	local: LocalNode,
	cancel: CancellationToken,
	events: UnboundedSender<Event>,
	catalog: watch::Receiver<Catalog>,
	last_own_version: PeerEntryVersion,
	messages_in: UnboundedChannel<AnnouncementMessage>,
	messages_out: UnboundedChannel<AnnouncementMessage>,
	neighbors_count: Arc<AtomicUsize>,
	dials: UnboundedReceiver<Vec<PeerId>>,
}

impl WorkerLoop {
	async fn spawn(mut self) -> Result<(), Error> {
		// Ensure that the local node is online and has all protocols installed
		// and addresses resolved.
		self.local.online().await;

		let topic_id = self.local.network_id().into();
		let (mut topic_tx, mut topic_rx) = self
			.gossip
			.subscribe(topic_id, self.config.bootstrap_peers.clone())
			.await?
			.split();

		loop {
			tokio::select! {
				// Network is terminating, exit the loop
				() = self.cancel.cancelled() => {
					tracing::debug!(
						network = %self.local.network_id(),
						"Discovery announcement protocol terminating"
					);
					return Ok(());
				}

				// There is an outbound message to broadcast and we have neighbors
				Some(outbound) = self.messages_out.recv(), if topic_rx.is_joined() => {
					self.broadcast_message(
						&mut topic_tx,
						&mut topic_rx,
						outbound
					).await?;
				}

				// There is an inbound message received from gossip broadcast
				Some(inbound) = self.messages_in.recv() => {
					self.on_message_received(inbound);
				}

				// The gossip topic has an event
				gossip_event = topic_rx.next() => {
					self.on_topic_rx(gossip_event, &mut topic_rx, &mut topic_tx).await?;
				}

				// The local peer entry has been updated
				Ok(()) = self.catalog.changed() => {
					self.on_catalog_update();
				}

				// Manual dial request
				Some(peers) = self.dials.recv() => {
					self.dial_peers(peers, &mut topic_tx).await;
				}
			}
		}
	}

	/// Initializes the gossip topic for discovery.
	///
	/// This joins an iroh-gossip topic based on the network ID.
	async fn join_gossip_topic(&self) -> Result<GossipTopic, Error> {
		let topic_id = self.local.network_id().into();
		let bootstrap = self.config.bootstrap_peers.clone();
		let topic = self.gossip.subscribe(topic_id, bootstrap).await?;
		Ok(topic)
	}

	/// Handles events from the iroh gossip topic in their raw form.
	///
	/// This method processes low-level gossip events such as connection drops,
	/// errors, and received messages, delegating to specific handlers as needed.
	///
	/// When a topic is closed, it attempts to rejoin the topic.
	async fn on_topic_rx(
		&mut self,
		gossip_event: Option<Result<GossipEvent, GossipError>>,
		topic_rx: &mut GossipReceiver,
		topic_tx: &mut GossipSender,
	) -> Result<(), Error> {
		match gossip_event {
			None | Some(Err(GossipError::Closed { .. })) => {
				// topic connection dropped, re-join
				tracing::warn!(
					network = %self.local.network_id(),
					"Gossip topic connection closed, re-joining"
				);
				self.neighbors_count.store(0, Ordering::SeqCst);
				self.rejoin_topic(topic_tx, topic_rx).await?;
			}
			Some(Err(e)) => {
				tracing::warn!(
					network = %self.local.network_id(),
					"Gossip topic error: {e}"
				);
			}
			Some(Ok(event)) => {
				self.on_gossip_event(event);
			}
		}

		Ok(())
	}

	/// Handle gossip-level events.
	///
	/// This method handles the happy-path gossip events, such as new neighbors
	/// joining and messages being received.
	fn on_gossip_event(&mut self, event: GossipEvent) {
		tracing::trace!(
			network = %self.local.network_id(),
			peer_id = %Short(&self.local.id()),
			event = ?event,
			"Received gossip event"
		);

		match event {
			GossipEvent::NeighborUp(_) => {
				self.neighbors_count.fetch_add(1, Ordering::SeqCst);
				self.broadcast_self_info();
			}
			GossipEvent::NeighborDown(_) => {
				self.neighbors_count.fetch_sub(1, Ordering::SeqCst);
			}
			GossipEvent::Received(message) => {
				let Ok(decoded) =
					decode_from_std_read(&mut message.content.reader(), standard())
				else {
					tracing::warn!(
						network = %self.local.network_id(),
						"Failed to decode announcement message"
					);
					// todo: Ban peer due to protocol violation
					return;
				};

				self.on_message_received(decoded);
			}
			GossipEvent::Lagged => {
				self.neighbors_count.store(0, Ordering::SeqCst);
			}
		}
	}

	/// A message has been received from the gossip topic.
	fn on_message_received(&mut self, message: AnnouncementMessage) {
		match message {
			AnnouncementMessage::OwnEntryUpdate(entry) => {
				tracing::trace!(
						info = ?entry,
						network = %self.local.network_id(),
						"received peer entry update announcement"
				);

				// Update local state or catalog as needed
				let _ = self.events.send(Event::PeerEntryReceived(entry));
			}
		}
	}

	/// Handles updates to the local peer entry in the catalog.
	fn on_catalog_update(&mut self) {
		let current_local_version = self.catalog.borrow().local().update_version();
		if current_local_version > self.last_own_version {
			self.broadcast_self_info();
			self.last_own_version = current_local_version;
		}
	}

	/// Broadcasts the latest version of the local peer entry to the gossip topic.
	///
	/// If the local node is not connected to at least one gossip neighbor, this
	/// function returns early without broadcasting.
	fn broadcast_self_info(&self) {
		let entry = self.catalog.borrow().local().clone();

		tracing::debug!(
			network = %self.local.network_id(),
			entry = ?Pretty(&entry),
			"broadcasting local peer info update"
		);

		self
			.messages_out
			.send(AnnouncementMessage::OwnEntryUpdate(entry));
	}

	/// Broadcasts an announcement message to the gossip topic.
	///
	/// This method checks if there are any connected neighbors before
	/// broadcasting. If there are no neighbors, it defers the broadcast by
	/// re-queuing the message for later sending.
	///
	/// If the topic connection is closed, it attempts to re-join the topic.
	async fn broadcast_message(
		&mut self,
		topic_tx: &mut GossipSender,
		topic_rx: &mut GossipReceiver,
		message: AnnouncementMessage,
	) -> Result<(), Error> {
		if !topic_rx.is_joined() {
			tracing::debug!(
				network = %self.local.network_id(),
				"not connected to any gossip neighbors, \
				 deferring announcement broadcast"
			);

			// Re-queue the message for later retry
			self.messages_out.send(message);
			return Ok(());
		}

		if let Err(e) = topic_tx
			.broadcast(
				encode_to_vec(&message, standard())
					.expect("AnnouncementMessage Encoding failed")
					.into(),
			)
			.await
		{
			tracing::warn!(
				error = %e,
				network = %self.local.network_id(),
				message = ?message,
				"failed to broadcast announcement message"
			);

			if matches!(e, GossipError::Closed { .. }) {
				// topic connection dropped, re-join
				tracing::warn!(
					network = %self.local.network_id(),
					"gossip topic connection closed, re-joining"
				);

				self.rejoin_topic(topic_tx, topic_rx).await?;
			}
		} else {
			let neighbor_count = topic_rx.neighbors().count();
			tracing::trace!(
				network = %self.local.network_id(),
				"broadcasted announcement message to {neighbor_count} neighbors"
			);
		}
		Ok(())
	}

	async fn dial_peers(&self, peers: Vec<PeerId>, topic_tx: &mut GossipSender) {
		tracing::debug!(
			network = %self.local.network_id(),
			peers = ?peers,
			"Dialing peers"
		);

		if let Err(e) = topic_tx.join_peers(peers).await {
			tracing::warn!(
				error = %e,
				network = %self.local.network_id(),
				"Failed to dial peers via gossip topic"
			);
		}
	}

	/// This method is invoked when the gossip topic connection is closed.
	/// It attempts to re-join the topic to restore connectivity.
	async fn rejoin_topic(
		&mut self,
		topic_tx: &mut GossipSender,
		topic_rx: &mut GossipReceiver,
	) -> Result<(), Error> {
		let (new_topic_tx, new_topic_rx) = self.join_gossip_topic().await?.split();
		*topic_tx = new_topic_tx;
		*topic_rx = new_topic_rx;
		Ok(())
	}
}

/// Wire format for announcement messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
enum AnnouncementMessage {
	/// Broadcasted when a peer updates its own entry.
	OwnEntryUpdate(SignedPeerEntry),
}
