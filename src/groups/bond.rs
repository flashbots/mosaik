use {
	crate::{
		Groups,
		UniqueId,
		discovery::SignedPeerEntry,
		groups::{
			Config,
			Error,
			consensus::ConsensusMessage,
			error::{InvalidProof, Timeout},
			group::GroupState,
			wire::{BondMessage, HandshakeEnd, HandshakeStart},
		},
		network::{
			Cancelled,
			CloseReason,
			UnexpectedClose,
			UnknownPeer,
			link::{Link, RecvError, SendError},
		},
		primitives::{Short, UnboundedChannel},
	},
	core::{fmt, pin::pin, time::Duration},
	iroh::endpoint::{ApplicationClose, ConnectionError},
	rand::random,
	std::{sync::Arc, time::Instant},
	tokio::{
		sync::{
			Notify,
			futures::OwnedNotified,
			mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
			watch,
		},
		time::{Interval, MissedTickBehavior, interval, timeout},
	},
	tokio_util::sync::CancellationToken,
};

/// A unique identifier for a bond connection between the local node and a
/// remote peer in the group. This value is derived from the shared random
/// between the two peers during bond establishment and is used to correlate the
/// bond connection with the corresponding peer entry in the discovery catalog
/// and to identify the bond in logs and metrics.
///
/// The bond id should be identical on both sides of the bond connection.
pub type BondId = UniqueId;

#[derive(Debug, Clone)]
pub enum BondEvent {
	Connected,
	ConsensusMessage(ConsensusMessage),
	Terminated(ApplicationClose),
}

pub type BondEvents = UnboundedReceiver<BondEvent>;

/// Handle to a bond with another peer in a group.
#[derive(Clone)]
pub struct Bond {
	/// A unique identifier for this bond connection.
	///
	/// This value is identical on both sides of the bond connection.
	id: BondId,

	/// Cancellation token for terminating the bond's main loop
	/// and to check for cancellation status.
	cancel: CancellationToken,

	/// Channel for sending commands to the bond worker loop.
	commands_tx: UnboundedSender<Command>,

	/// Watch channel for observing updates to the remote peer's discovery entry.
	peer: watch::Receiver<SignedPeerEntry>,
}

impl fmt::Display for Bond {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			f,
			"Bond(peer={}, id={})",
			Short(self.peer.borrow().id()),
			Short(self.id)
		)
	}
}

/// Public API
impl Bond {
	/// Closes the bond connection to the remote peer with the provided
	/// application-level close reason.
	pub async fn close(self, reason: impl CloseReason) {
		let _ = self.commands_tx.send(Command::Close(reason.into()));
		self.cancel.cancelled().await;
	}

	/// Returns the most recent peer entry for the remote peer in the bond.
	pub fn peer(&self) -> SignedPeerEntry {
		self.peer.borrow().clone()
	}

	/// Returns the unique identifier for this bond connection.
	/// This value is derived from the shared random between the two peers during
	/// bond establishment and is used to correlate the bond connection with the
	/// corresponding peer entry in the discovery catalog and to identify the bond
	/// in logs and metrics.
	pub const fn id(&self) -> BondId {
		self.id
	}
}

/// Internal API
impl Bond {
	/// Sends a wire message over the bond connection to the remote peer.
	pub(super) fn send(&self, message: BondMessage) {
		let _ = self.commands_tx.send(Command::SendMessage(message));
	}
}

/// Represents a direct connection between the local node and another peer in
/// the group. Each node in the group maintains a bond with every other node in
/// the group, and over these bonds group messages are exchanged. Bonds are
/// long-lived connections.
///
/// Notes:
///
/// - Bonds are bidirectional. Once established, both peers can send messages to
///   each other over the same bond.
///
/// - Bonds are persistent. If a bond drops, the local node will attempt to
///   re-establish it.
///
/// - Bonds use heartbeats to monitor the health and liveness of the connection
///   if no messages are being exchanged within a `heartbeat_interval`.
///
/// - Bonded peers receive low-latency updates about changes to each other's
///   discovery peer entries over the bond connection outside of the normal
///   discovery mechanisms to ensure timely propagation of changes to all group
///   members.
pub struct BondWorker {
	/// Reference to the shared group state managing this bond.
	group: Arc<GroupState>,

	/// The peer entry representing the remote peer in the group.
	peer: watch::Sender<SignedPeerEntry>,

	/// Channel for receiving commands to control the bond by the handle.
	commands: UnboundedChannel<Command>,

	/// Underlying transport link for sending and receiving messages over the
	/// bond connection.
	link: Link<Groups>,

	/// Pending outbound messages to be sent over the bond.
	pending_sends: UnboundedChannel<BondMessage>,

	/// Manages heartbeats sent over the bond to ensure liveness.
	heartbeat: Heartbeat,

	/// Cancellation token for terminating the bond's main loop.
	/// It gets implicitly cancelled when the group is shut down or during
	/// network termination.
	cancel: CancellationToken,

	/// Channel for sending bond events to the group managing it.
	events_tx: UnboundedSender<BondEvent>,

	/// The reason for closing the bond connection when it is terminated.
	///
	/// This is the application-level reason that will be sent to the remote peer
	/// when closing the link over the wire.
	close_reason: ApplicationClose,
}

impl BondWorker {
	pub fn spawn(
		group: Arc<GroupState>,
		peer: SignedPeerEntry,
		link: Link<Groups>,
	) -> (Bond, BondEvents) {
		let mut link = link;
		let (peer, peer_rx) = watch::channel(peer);
		let cancel = group.cancel.child_token();
		let heartbeat = Heartbeat::new(&group.config);
		let commands = UnboundedChannel::default();
		let commands_tx = commands.sender().clone();
		let (events_tx, events_rx) = unbounded_channel();
		link.replace_cancel_token(cancel.clone());

		let bond_id = link.shared_random("bond_id");

		let bond = BondWorker {
			group,
			peer,
			link,
			heartbeat,
			commands,
			events_tx,
			cancel: cancel.clone(),
			pending_sends: UnboundedChannel::default(),
			close_reason: Cancelled.into(),
		};

		tokio::spawn(bond.run());

		(
			Bond {
				cancel,
				commands_tx,
				id: bond_id,
				peer: peer_rx,
			},
			events_rx,
		)
	}
}

impl BondWorker {
	/// Main loop for managing the bond connection.
	async fn run(mut self) {
		let mut link_dropped = pin!(self.link.closed());
		let mut heartbeat_fail = pin!(self.heartbeat.failed());
		self.events_tx.send(BondEvent::Connected).ok();

		loop {
			tokio::select! {
				() = self.cancel.cancelled() => {
					break;
				}

				// transport link dropped
				reason = &mut link_dropped, if !self.cancel.is_cancelled() => {
					self.on_link_closed(reason);
				}

				// incoming wire message
				result = self.link.recv::<BondMessage>(), if !self.cancel.is_cancelled() => {
					self.on_next_recv(result);
				}

				// push pending outbound message
				Some(message) = self.pending_sends.recv(), if !self.cancel.is_cancelled() => {
					let res = self.link.send(&message).await;
					self.on_send_complete(res);
				}

				// Heartbeat tick
				() = self.heartbeat.tick(), if !self.cancel.is_cancelled() => {
					self.on_heartbeat_tick();
				}

				// Command from bond handle
				Some(cmd) = self.commands.recv(), if !self.cancel.is_cancelled() => {
					self.on_command(cmd);
				}

				// Heartbeat failure
				() = &mut heartbeat_fail, if !self.cancel.is_cancelled() => {
					self.on_heartbeat_failed();
					self.close_reason = Timeout.into();
				}
			}
		}

		self.link.close(self.close_reason.clone()).await.ok();

		self
			.events_tx
			.send(BondEvent::Terminated(self.close_reason))
			.ok();
	}

	fn on_command(&mut self, command: Command) {
		match command {
			Command::Close(reason) => {
				self.cancel.cancel();
				self.close_reason = reason;
			}
			Command::SendMessage(message) => {
				self.pending_sends.send(message);
			}
		}
	}

	/// Called when the next message is received from the link.
	/// Resets the heartbeat timer.
	fn on_next_recv(&mut self, result: RecvResult) {
		match result {
			Ok(message) => {
				self.heartbeat.reset();
				match message {
					BondMessage::Pong => {}
					BondMessage::Ping => self.on_heartbeat_ping(),
					BondMessage::PeerEntryUpdate(entry) => {
						self.on_peer_entry_update(*entry);
					}
					BondMessage::BondFormed(peer) => {
						self.on_bond_formed(*peer);
					}
					BondMessage::Consensus(message) => {
						self
							.events_tx
							.send(BondEvent::ConsensusMessage(message))
							.ok();
					}
				}
			}
			Err(e) => {
				tracing::debug!(
					error = %e,
					network = %self.group.local.network_id(),
					peer = %Short(self.link.remote_id()),
					group = %Short(self.group.key.id()),
					"recv",
				);

				if !e.is_cancelled() {
					self.close_reason = e.close_reason() //.
						.cloned().unwrap_or(UnexpectedClose.into());
				}

				self.cancel.cancel();
			}
		}
	}

	/// Called after sending a message over the link.
	/// Ensures that the link is still healthy and closes it on error.
	fn on_send_complete(&mut self, result: SendResult) {
		if let Err(e) = result {
			tracing::debug!(
				error = %e,
				network = %self.group.local.network_id(),
				peer = %Short(self.link.remote_id()),
				group = %Short(self.group.key.id()),
				"send",
			);

			if !e.is_cancelled() {
				self.close_reason = e.close_reason() //.
					.cloned().unwrap_or(UnexpectedClose.into());
			}

			self.cancel.cancel();
		}
	}

	/// Received an update about a change to a group member's peer entry.
	fn on_peer_entry_update(&self, entry: SignedPeerEntry) {
		if self.group.discovery.feed(entry.clone()) {
			tracing::trace!(
				network = %self.group.local.network_id(),
				peer = %Short(self.link.remote_id()),
				group = %Short(self.group.key.id()),
				"peer entry update received",
			);

			self.peer.send_modify(|existing| *existing = entry);
		}
	}

	/// Called when a remote peer informs us that it has formed a bond with some
	/// other peer in the group.
	fn on_bond_formed(&self, entry: SignedPeerEntry) {
		self
			.group
			.send_command(super::group::Command::Connect(entry));
	}

	/// Called when the underlying transport link is dropped.
	fn on_link_closed(&mut self, reason: Result<(), ConnectionError>) {
		if let Err(ConnectionError::ApplicationClosed(e)) = reason {
			self.close_reason = e;
		}
		self.cancel.cancel();
	}
}

impl BondWorker {
	/// Initiates the process of creating a new bond connection to a remote
	/// peer in the group.
	///
	/// This happens in response to discovering a new peer in the group via
	/// the discovery catalog. This method is called only for peers that are
	/// already known in the discovery catalog.
	pub async fn create(
		group: Arc<GroupState>,
		peer: SignedPeerEntry,
	) -> Result<(Bond, BondEvents), Error> {
		tracing::trace!(
			network = %group.local.network_id(),
			peer = %Short(peer.id()),
			group = %Short(group.key.id()),
			"initiating peer bond",
		);

		// attempt to establish a new wire link to the remote peer
		let mut link = group
			.local
			.connect_with_cancel::<Groups>(
				peer.address().clone(),
				group.cancel.child_token(),
			)
			.await
			.map_err(|e| Error::Link(e.into()))?;

		// prepare a handshake message with our proof of knowledge of the group
		// secret and send it to the remote peer over the newly established link
		link
			.send(&HandshakeStart {
				network_id: *group.local.network_id(),
				group_id: *group.key.id(),
				proof: group.key.generate_proof(&link, group.local.id()),
				bonds: group.bonds.iter().map(|b| b.peer().clone()).collect(),
			})
			.await
			.map_err(|e| Error::Link(e.into()))?;

		// After sending our handshake with a proof of knowledge of the group
		// secret, wait for the accepting peer to respond with its own proof of
		// knowledge of the group secret.
		let Ok(recv_result) = timeout(
			group.config.handshake_timeout, //
			link.recv::<HandshakeEnd>(),
		)
		.await
		else {
			tracing::debug!(
				network = %group.local.network_id(),
				peer = %Short(peer.id()),
				group = %Short(group.key.id()),
				"handshake timeout waiting for bond confirmation",
			);

			return Err(Error::Link(RecvError::Cancelled.into()));
		};

		let confirm = match recv_result {
			Ok(resp) => resp,
			Err(e) => match e.close_reason() {
				// the remote peer closed the link during handshake because the local
				// node is not known in its discovery catalog. Trigger full discovery
				// catalog sync.
				Some(reason) if reason == UnknownPeer => {
					// trigger full catalog sync and retry bonding
					group
						.discovery
						.sync_with(peer.address().clone())
						.await
						.map_err(Error::Discovery)?;

					// retry creating the bond after syncing the catalog
					return Box::pin(Self::create(group, peer)).await;
				}
				// The remote peer rejected our authentication proof.
				// This is an unrecoverable error in the current set of authorization
				// schemes and most likely indicates an incompatible version of the peer
				// or a malicious actor.
				Some(reason) if reason == InvalidProof => {
					tracing::warn!(
						network = %group.local.network_id(),
						peer = %Short(peer.id()),
						group = %Short(group.key.id()),
						"remote peer rejected bond handshake due to invalid proof",
					);

					link
						.close(InvalidProof)
						.await
						.map_err(|e| Error::Link(e.into()))?;

					return Err(Error::InvalidGroupKeyProof);
				}
				// Bonding failed for some other reason.
				_ => return Err(Error::Link(e.into())),
			},
		};

		// validate the accepting peer's proof of knowledge of the group secret
		if !group.key.validate_proof(&link, confirm.proof) {
			tracing::warn!(
				network = %group.local.network_id(),
				peer = %Short(peer.id()),
				group = %Short(group.key.id()),
				"remote peer provided invalid group secret proof",
			);

			link
				.close(InvalidProof)
				.await
				.map_err(|e| Error::Link(e.into()))?;

			return Err(Error::InvalidGroupKeyProof);
		}

		// attempt to connect to all existing peers in the group known to the
		// accepting node
		for peer in confirm.bonds {
			group.bond_with(peer);
		}

		Ok(BondWorker::spawn(group, peer, link))
	}

	/// Accepts an incoming bond connection for this group.
	///
	/// This is called by the group's protocol handler when a new connection
	/// is established  in [`Listener::accept`].
	///
	/// By the time this method is called:
	/// - The network id has already been verified to match the local node's
	///   network id.
	/// - The group id has already been verified to match this group's id.
	/// - The presence of the remote peer in the local discovery catalog is
	///   verified.
	/// - The authentication proof has not been verified yet.
	pub async fn accept(
		group: Arc<GroupState>,
		link: Link<Groups>,
		peer: SignedPeerEntry,
		handshake: HandshakeStart,
	) -> Result<(Bond, BondEvents), Error> {
		let mut link = link;

		// verify the remote peer's proof of knowledge of the group secret
		if !group.key.validate_proof(&link, handshake.proof) {
			tracing::warn!(
				network = %group.local.network_id(),
				peer = %Short(peer.id()),
				group = %Short(group.key.id()),
				"remote peer provided invalid group secret proof",
			);

			link
				.close(InvalidProof)
				.await
				.map_err(|e| Error::Link(e.into()))?;

			return Err(Error::InvalidGroupKeyProof);
		}

		// After verifying the remote peer's proof of knowledge of the group secret,
		// respond with our own proof of knowledge of the group secret.
		let proof = group.key.generate_proof(&link, group.local.id());
		let existing = group.bonds.iter().map(|b| b.peer().clone()).collect();
		let resp = HandshakeEnd {
			proof,
			bonds: existing,
		};
		link.send(&resp).await.map_err(|e| Error::Link(e.into()))?;

		// attempt to connect to all existing peers in the group known to the
		// initiating node
		for peer in handshake.bonds {
			group.bond_with(peer);
		}

		Ok(BondWorker::spawn(group, peer, link))
	}
}

enum Command {
	/// Closes the bond connection with the provided application-level reason.
	/// The oneshot sender is used to signal completion of the close operation.
	Close(ApplicationClose),

	/// Sends a wire message over the bond connection to the remote peer.
	SendMessage(BondMessage),
}

type SendResult = Result<usize, SendError>;
type RecvResult = Result<BondMessage, RecvError>;

/// Heartbeats are messages that are sent periodically over a bond connection to
/// the remote peer when no other messages are being exchanged to ensure that
/// the connection is still alive and responsive.
///
/// When a heartbeat is sent, the heartbeat timer is started, and if no message
/// is received from the remote peer before the next heartbeat interval, a
/// missed heartbeat is recorded. If the number of consecutive missed heartbeats
/// exceeds the configured maximum, then the heartbeat is considered failed.
///
/// The heartbeat message in times of idleness is a simple `Ping` message, if
/// the remote peer is alive it should respond with a `Pong` message or any
/// other message which will also reset the heartbeat timer.
pub struct Heartbeat {
	tick: Interval,
	last_recv: Instant,
	missed: usize,
	max_missed: usize,
	alert: Arc<Notify>,
	base: Duration,
	jitter: Duration,
}

impl Heartbeat {
	pub fn new(config: &Config) -> Self {
		let next_tick_at = Self::next_tick_at(
			&config.heartbeat_interval, //
			&config.heartbeat_jitter,
		);

		let mut tick = interval(next_tick_at);
		tick.set_missed_tick_behavior(MissedTickBehavior::Delay);

		Self {
			tick,
			missed: 0,
			last_recv: Instant::now(),
			max_missed: config.max_missed_heartbeats,
			alert: Arc::new(Notify::new()),
			base: config.heartbeat_interval,
			jitter: config.heartbeat_jitter,
		}
	}

	/// Completes when the next heartbeat is due.
	pub async fn tick(&mut self) {
		self.tick.tick().await;

		let max_gap = self.base.saturating_add(self.jitter);
		if self.last_recv.elapsed() > max_gap {
			self.missed += 1;
		}

		if self.missed >= self.max_missed {
			self.alert.notify_waiters();
		}
	}

	/// Resets the heartbeat timer after any received message over the bond link.
	pub fn reset(&mut self) {
		self
			.tick
			.reset_after(Self::next_tick_at(&self.base, &self.jitter));

		self.last_recv = Instant::now();
		self.missed = 0;
	}

	/// Completes when the heartbeat has failed due to too many missed heartbeats.
	pub fn failed(&self) -> OwnedNotified {
		Arc::clone(&self.alert).notified_owned()
	}

	fn next_tick_at(base: &Duration, jitter: &Duration) -> Duration {
		#[expect(clippy::cast_possible_truncation)]
		let millis_sub = random::<u64>() % jitter.as_millis() as u64;
		let sub = Duration::from_millis(millis_sub);
		base.saturating_sub(sub)
	}
}

/// Heartbeat-related event handlers.
impl BondWorker {
	/// Called when the heartbeat timer ticks.
	/// If there are no outbound messages pending, a heartbeat `Ping` is sent.
	pub(super) fn on_heartbeat_tick(&self) {
		if self.pending_sends.is_empty() {
			tracing::trace!(
				network = %self.group.local.network_id(),
				peer = %Short(self.link.remote_id()),
				group = %Short(self.group.key.id()),
				rtt = ?self.link.rtt(),
				"sending heartbeat ping",
			);

			self.pending_sends.send(BondMessage::Ping);
		}
	}

	/// Called when the heartbeat has failed due to too many missed heartbeats.
	pub(super) fn on_heartbeat_failed(&mut self) {
		tracing::warn!(
			network = %self.group.local.network_id(),
			peer = %Short(self.link.remote_id()),
			group = %Short(self.group.key.id()),
			"heartbeat failed: too many missed heartbeats",
		);

		self.cancel.cancel();
	}

	pub(super) fn on_heartbeat_ping(&self) {
		tracing::trace!(
			network = %self.group.local.network_id(),
			peer = %Short(self.link.remote_id()),
			group = %Short(self.group.key.id()),
			rtt = ?self.link.rtt(),
			"received heartbeat ping, sending pong",
		);

		self.pending_sends.send(BondMessage::Pong);
	}
}
