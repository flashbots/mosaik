use {
	super::protocol::BondMessage,
	crate::{
		Groups,
		discovery::SignedPeerEntry,
		groups::{
			Bond,
			bond::{BondEvent, BondEvents, heartbeat::Heartbeat},
			error::Timeout,
			group::GroupState,
		},
		network::{link::*, *},
		primitives::{Short, UnboundedChannel},
	},
	bytes::Bytes,
	core::pin::pin,
	iroh::endpoint::{ApplicationClose, ConnectionError},
	itertools::Either,
	std::sync::Arc,
	tokio::sync::{
		mpsc::{UnboundedSender, unbounded_channel},
		watch,
	},
	tokio_util::sync::CancellationToken,
};

/// Commands sent by the `Bond` handle to the `BondWorker` to control its
/// behavior and send messages over the bond connection.
pub(super) enum Command {
	/// Closes the bond connection with the provided application-level reason.
	/// The oneshot sender is used to signal completion of the close operation.
	Close(ApplicationClose),

	/// Sends a wire message over the bond connection to the remote peer.
	SendMessage(BondMessage),

	/// Sends a raw pre-encoded message over the bond connection to the remote
	/// peer. This is used when the same message is being sent to multiple peers
	/// to avoid redundant encoding.
	SendRawMessage(Bytes),
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

	/// Channel for receiving commands to control the bond worker by the handle.
	commands: UnboundedChannel<Command>,

	/// Underlying transport link for sending and receiving messages over the
	/// bond connection.
	link: Link<Groups>,

	/// Pending outbound messages to be sent over the bond.
	pending_sends: UnboundedChannel<Either<BondMessage, Bytes>>,

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
					self.send_message(message).await;
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
				self.pending_sends.send(Either::Left(message));
			}
			Command::SendRawMessage(message) => {
				self.pending_sends.send(Either::Right(message));
			}
		}
	}

	async fn send_message(&mut self, message: Either<BondMessage, Bytes>) {
		let res = match message {
			Either::Left(msg) => self.link.send(&msg).await,
			Either::Right(raw) => unsafe { self.link.send_raw(raw).await },
		};

		self.on_send_complete(res);
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
						self.on_bond_formed_notification(*peer);
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
	fn on_bond_formed_notification(&self, entry: SignedPeerEntry) {
		self.group.bond_with(entry);
	}

	/// Called when the underlying transport link is dropped.
	fn on_link_closed(&mut self, reason: Result<(), ConnectionError>) {
		if let Err(ConnectionError::ApplicationClosed(e)) = reason {
			self.close_reason = e;
		}
		self.cancel.cancel();
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

			self.enqueue_message(BondMessage::Ping);
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

		self.enqueue_message(BondMessage::Pong);
	}
}

// commands
impl BondWorker {
	fn enqueue_message(&self, message: BondMessage) {
		self.pending_sends.send(Either::Left(message));
	}
}

type SendResult = Result<usize, SendError>;
type RecvResult = Result<BondMessage, RecvError>;
