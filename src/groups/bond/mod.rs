use {
	crate::{
		Groups,
		discovery::SignedPeerEntry,
		groups::{
			bond::heartbeat::Heartbeat,
			error::{Error, Timeout},
			group::GroupState,
		},
		network::{
			Cancelled,
			CloseReason,
			UnexpectedClose,
			link::{Link, RecvError, SendError},
		},
		primitives::{Short, UnboundedChannel},
	},
	core::{fmt, pin::pin},
	iroh::endpoint::ApplicationClose,
	serde::{Deserialize, Serialize},
	std::sync::Arc,
	tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
	tokio_util::sync::CancellationToken,
};

mod handshake;
mod heartbeat;

use {iroh::endpoint::ConnectionError, tokio::sync::watch};

#[derive(Debug, Clone)]
pub enum BondEvent {
	Connected,
	Terminated(ApplicationClose),
}

pub type BondEvents = UnboundedReceiver<BondEvent>;

/// Handle to a bond with another peer in a group.
#[derive(Clone)]
pub struct Bond {
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
		write!(f, "Bond(peer={})", Short(self.peer.borrow().id()))
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
}

/// Internal API
impl Bond {
	/// Sends a wire message over the bond connection to the remote peer.
	pub(super) fn send(&self, message: WireMessage) {
		let _ = self.commands_tx.send(Command::SendMessage(message));
	}
}

/// Represents a direct connection between the local node and another peer in
/// the group. Each node in the group maintains a bond with every other node in
/// the group, and it is over these bonds that group messages are exchanged.
/// Bonds are long-lived connections that are re-established if they drop.
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
	pending_sends: UnboundedChannel<WireMessage>,

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
				result = self.link.recv::<WireMessage>(), if !self.cancel.is_cancelled() => {
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
					WireMessage::Ping => self.on_heartbeat_ping(),
					WireMessage::Pong => {}
					WireMessage::PeerEntryUpdate(entry) => {
						self.on_peer_entry_update(*entry);
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

	/// Called when the underlying transport link is dropped.
	fn on_link_closed(&mut self, reason: Result<(), ConnectionError>) {
		if let Err(ConnectionError::ApplicationClosed(e)) = reason {
			self.close_reason = e;
		}
		self.cancel.cancel();
	}
}

enum Command {
	/// Closes the bond connection with the provided application-level reason.
	/// The oneshot sender is used to signal completion of the close operation.
	Close(ApplicationClose),

	/// Sends a wire message over the bond connection to the remote peer.
	SendMessage(WireMessage),
}

type SendResult = Result<usize, SendError>;
type RecvResult = Result<WireMessage, RecvError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WireMessage {
	Ping,
	Pong,
	PeerEntryUpdate(Box<SignedPeerEntry>),
}
