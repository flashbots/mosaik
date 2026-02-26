use {
	super::{Catalog, Config, Error, PeerEntryVersion, SignedPeerEntry},
	crate::{
		NetworkId,
		PeerId,
		Signature,
		discovery::PeerEntry,
		network::{LocalNode, link::Protocol},
		primitives::{
			IntoIterOrSingle,
			Pretty,
			Short,
			UnboundedChannel,
			deserialize,
			serialize,
		},
	},
	chrono::Utc,
	core::{
		sync::atomic::{AtomicUsize, Ordering},
		time::Duration,
	},
	futures::StreamExt,
	iroh::{
		EndpointAddr,
		address_lookup::AddressLookup,
		protocol::ProtocolHandler,
	},
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
		oneshot,
		watch,
	},
	tokio_util::sync::CancellationToken,
	tracing::error,
};

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum Event {
	/// A valid and signed peer entry has been updated.
	PeerEntryReceived(SignedPeerEntry),

	/// A peer has gracefully departed the network.
	PeerDeparted(PeerId, PeerEntryVersion),
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
///
/// - Announcements are broadcasted over the gossip topic to all subscribed
///   peers.
///
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
	local: LocalNode,
	network_id: NetworkId,
	events: UnboundedReceiver<Event>,
	dials: UnboundedSender<(Vec<EndpointAddr>, oneshot::Sender<()>)>,
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
		let network_id = *local.network_id();
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
					network_id = %network_id,
					"Unrecoverable error in discovery protocol, terminating network"
				);

				// Trigger network termination
				cancel.cancel();
			}
		});

		Self {
			gossip,
			local,
			network_id,
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
	pub async fn dial<V>(&self, peers: impl IntoIterOrSingle<EndpointAddr, V>) {
		let (tx, rx) = oneshot::channel::<()>();

		let peers = peers.iterator().into_iter().collect::<Vec<_>>();
		for peer in &peers {
			self
				.local
				.endpoint()
				.address_lookup()
				.publish(&peer.clone().into());
		}

		self.dials.send((peers, tx)).ok();
		let _ = rx.await;
	}

	/// A hint to observe a peer.
	///
	/// This is useful when a peer does not have any connected gossip neighbors
	/// but it does a full catalog sync with another peer or learns in any other
	/// way about another peer and there are new potential peers to connect to.
	pub fn observe(&self, peer: &PeerEntry) {
		// Ignore peers from different networks
		if peer.network_id() != self.network_id {
			return;
		}

		self.local.observe(peer.address());

		if self.neighbors_count.load(Ordering::SeqCst) == 0 {
			let (tx, _) = oneshot::channel::<()>();
			self.dials.send((vec![peer.address().clone()], tx)).ok();
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
	dials: UnboundedReceiver<(Vec<EndpointAddr>, oneshot::Sender<()>)>,
}

impl WorkerLoop {
	async fn spawn(mut self) -> Result<(), Error> {
		// Ensure that the local node is online and has all protocols installed
		// and addresses resolved.
		self.local.online().await;

		// add bootstrap peers to the addressing system
		self.local.observe(self.config.bootstrap_peers.iter());
		let peer_ids = self.config.bootstrap_peers_ids();
		let topic_id = self.local.network_id().into();
		let (mut topic_tx, mut topic_rx) =
			self.gossip.subscribe(topic_id, peer_ids).await?.split();

		loop {
			tokio::select! {
				// Network is terminating, exit the loop
				() = self.cancel.cancelled() => {
					self.shutdown(&topic_tx, &topic_rx).await;
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

				// The local catalog has been updated
				Ok(()) = self.catalog.changed() => {
					self.on_catalog_update();
				}

				// Manual dial request
				Some((peers, tx)) = self.dials.recv() => {
					self.dial_peers(peers, &topic_tx, tx).await;
				}
			}
		}
	}

	/// Initializes the gossip topic for discovery.
	///
	/// This joins an iroh-gossip topic based on the network ID.
	async fn join_gossip_topic(&self) -> Result<GossipTopic, Error> {
		let topic_id = self.local.network_id().into();
		let bootstrap = self.config.bootstrap_peers_ids();
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
		&self,
		gossip_event: Option<Result<GossipEvent, GossipError>>,
		topic_rx: &mut GossipReceiver,
		topic_tx: &mut GossipSender,
	) -> Result<(), Error> {
		match gossip_event {
			None | Some(Err(GossipError::Closed { .. })) => {
				// topic connection dropped, re-join
				tracing::warn!(
					network = %self.local.network_id(),
					"announcement gossip network connection lost, attempting to re-join"
				);
				self.neighbors_count.store(0, Ordering::SeqCst);
				self.rejoin_topic(topic_tx, topic_rx).await?;
			}
			Some(Err(e)) => {
				tracing::warn!(
					error = %e,
					network = %self.local.network_id(),
					"announcement gossip network down"
				);
				self.neighbors_count.store(0, Ordering::SeqCst);
				self.rejoin_topic(topic_tx, topic_rx).await?;
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
	fn on_gossip_event(&self, event: GossipEvent) {
		match event {
			GossipEvent::NeighborUp(id) => {
				self.neighbors_count.fetch_add(1, Ordering::SeqCst);
				tracing::trace!(
					network = %self.local.network_id(),
					peer_id = %Short(&id),
					neighbors = self.neighbors_count.load(Ordering::SeqCst),
					"New gossip neighbor connected"
				);

				// Broadcast our own info to the new neighbor
				self.broadcast_self_info();
			}
			GossipEvent::NeighborDown(id) => {
				self.neighbors_count.fetch_sub(1, Ordering::SeqCst);
				tracing::trace!(
					network = %self.local.network_id(),
					peer_id = %Short(&id),
					neighbors = self.neighbors_count.load(Ordering::SeqCst),
					"Gossip neighbor disconnected"
				);
			}
			GossipEvent::Received(message) => {
				let Ok(decoded) = deserialize(&message.content) else {
					tracing::warn!(
						network = %self.local.network_id(),
						"failed to decode announcement message"
					);
					// todo: Ban peer due to protocol violation
					return;
				};

				self.on_message_received(decoded);
			}
			GossipEvent::Lagged => {
				// we lost track of some updates, put the system in a safe state
				// and re-sync the catalog with the next peer interaction
				self.neighbors_count.store(0, Ordering::SeqCst);
			}
		}
	}

	/// A message has been received from the gossip topic.
	fn on_message_received(&self, message: AnnouncementMessage) {
		match message {
			// a peer is announcing an updated version of its own entry
			AnnouncementMessage::OwnEntryUpdate(entry) => {
				if entry.network_id() != self.local.network_id() {
					tracing::trace!(
						peer_network = %Short(entry.network_id()),
						this_network = %Short(self.local.network_id()),
						"received peer entry from different network, ignoring"
					);
					return;
				}

				// Check if the update timestamp is within allowed drift
				let Ok(time_diff) = (Utc::now() - entry.updated_at()).abs().to_std()
				else {
					tracing::trace!(
						peer_id = %Short(&entry.id()),
						network = %Short(entry.network_id()),
						"ignoring discovery entry with invalid timestamp"
					);
					return;
				};

				if time_diff > self.config.max_time_drift {
					tracing::trace!(
						peer_id = %Short(&entry.id()),
						network = %Short(entry.network_id()),
						time_diff = ?time_diff,
						max_drift = ?self.config.max_time_drift,
						"ignoring discovery entry with stale timestamp"
					);
					return;
				}

				// Update local state or catalog as needed
				let _ = self.events.send(Event::PeerEntryReceived(entry));
			}

			// a peer is gracefully departing the network
			AnnouncementMessage::GracefulDeparture(departure) => {
				if !departure.has_valid_signature() {
					tracing::trace!(
						peer_id = %Short(&departure.peer_id),
						"received graceful departure with invalid signature, ignoring"
					);
					return;
				}

				let time_diff = (Utc::now() - departure.timestamp)
					.abs()
					.to_std()
					.unwrap_or(Duration::MAX);

				if time_diff > self.config.max_time_drift {
					tracing::trace!(
						peer_id = %Short(&departure.peer_id),
						time_diff = ?time_diff,
						max_drift = ?self.config.max_time_drift,
						"received graceful departure with invalid timestamp, ignoring"
					);
					return;
				}

				let _ = self.events.send(Event::PeerDeparted(
					departure.peer_id,
					departure.last_version,
				));
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
	///
	/// We distinguish between periodic and immediate broadcasts for logging
	/// purposes.
	fn broadcast_self_info(&self) {
		let entry = self.catalog.borrow().local().clone();
		let entry = entry
			.into_unsigned()
			.increment_version()
			.sign(self.local.secret_key())
			.expect("failed to sign local peer entry update");

		tracing::trace!(
			peer_info = ?Pretty(&entry),
			network = %self.local.network_id(),
			"broadcasting local"
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
		&self,
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

		if let Err(e) = topic_tx.broadcast(serialize(&message)).await {
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
					"announcement gossip network connection lost, attempting to re-join"
				);

				self.rejoin_topic(topic_tx, topic_rx).await?;
			}
		} else {
			// successfully broadcasted the message, update neighbors stats
			let neighbor_count = topic_rx.neighbors().count();
			self.neighbors_count.store(neighbor_count, Ordering::SeqCst);
		}

		Ok(())
	}

	async fn dial_peers(
		&self,
		peers: Vec<EndpointAddr>,
		topic_tx: &GossipSender,
		tx: oneshot::Sender<()>,
	) {
		self.local.observe(peers.iter());
		let peer_ids = peers.into_iter().map(|p| p.id).collect::<Vec<_>>();

		tracing::trace!(
			network = %self.local.network_id(),
			peers = %Short::iter(&peer_ids),
			"Dialing peers"
		);

		if let Err(e) = topic_tx.join_peers(peer_ids).await {
			tracing::warn!(
				error = %e,
				network = %self.local.network_id(),
				"failed to dial peers via announcement gossip network"
			);
		}

		let _ = tx.send(());
	}

	/// This method is invoked when the gossip topic connection is closed.
	/// It attempts to re-join the topic to restore connectivity.
	async fn rejoin_topic(
		&self,
		topic_tx: &mut GossipSender,
		topic_rx: &mut GossipReceiver,
	) -> Result<(), Error> {
		let (new_topic_tx, new_topic_rx) = self.join_gossip_topic().await?.split();
		*topic_tx = new_topic_tx;
		*topic_rx = new_topic_rx;
		Ok(())
	}

	/// Triggered when the network is shutting down.
	///
	/// This broadcasts a graceful departure message to inform peers
	/// of the impending disconnection.
	///
	/// It then waits for a configured duration to allow the message
	/// to propagate before completing the shutdown process.
	async fn shutdown(self, topic_tx: &GossipSender, topic_rx: &GossipReceiver) {
		tracing::trace!(
			network = %self.local.network_id(),
			peer_id = %Short(&self.local.id()),
			"Discovery announcement protocol shutting down"
		);

		if topic_rx.is_joined() {
			let goodbye = AnnouncementMessage::GracefulDeparture(
				GracefulDeparture::new(&self.local, self.last_own_version),
			);

			if let Err(e) = topic_tx.broadcast(serialize(&goodbye)).await {
				tracing::warn!(
					error = %e,
					network = %self.local.network_id(),
					"failed to broadcast graceful departure message"
				);
			} else {
				tracing::trace!(
					network = %self.local.network_id(),
					peer_id = %Short(&self.local.id()),
					"broadcasted graceful departure message"
				);

				// give the broadcasted message some time to propagate before
				// disconnecting from the gossip topic
				tokio::time::sleep(self.config.graceful_departure_window).await;
			}
		}
	}
}

/// Wire format for announcement messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
enum AnnouncementMessage {
	/// Broadcasted when a peer updates its own entry.
	OwnEntryUpdate(SignedPeerEntry),

	/// Broadcasted when a peer is gracefully departing the network.
	GracefulDeparture(GracefulDeparture),
}

/// A message indicating a peer is gracefully departing the network.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct GracefulDeparture {
	peer_id: PeerId,
	last_version: PeerEntryVersion,
	timestamp: chrono::DateTime<Utc>,
	signature: Signature,
}

impl GracefulDeparture {
	pub fn new(local: &LocalNode, last_version: PeerEntryVersion) -> Self {
		let timestamp = Utc::now();
		let mut hasher = blake3::Hasher::default();

		hasher.update(local.id().as_bytes());
		hasher.update(&last_version.0.to_be_bytes());
		hasher.update(&last_version.1.to_be_bytes());
		hasher.update(&timestamp.timestamp_millis().to_be_bytes());
		let hash = hasher.finalize();

		let signature = local.secret_key().sign(hash.as_bytes());

		Self {
			peer_id: local.id(),
			last_version,
			timestamp,
			signature,
		}
	}

	pub fn has_valid_signature(&self) -> bool {
		let mut hasher = blake3::Hasher::default();

		hasher.update(self.peer_id.as_bytes());
		hasher.update(&self.last_version.0.to_be_bytes());
		hasher.update(&self.last_version.1.to_be_bytes());
		hasher.update(&self.timestamp.timestamp_millis().to_be_bytes());
		let hash = hasher.finalize();

		self
			.peer_id
			.verify(hash.as_bytes(), &self.signature)
			.is_ok()
	}
}
