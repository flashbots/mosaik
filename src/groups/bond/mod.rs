use {
	crate::{
		Groups,
		discovery::PeerEntry,
		groups::group::GroupState,
		network::{CloseReason, link::Link},
		primitives::Short,
	},
	core::{
		pin::Pin,
		task::{Context, Poll},
	},
	futures::Stream,
	iroh::endpoint::ApplicationClose,
	std::sync::Arc,
	tokio::sync::{
		mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
		oneshot,
	},
	tokio_util::sync::CancellationToken,
};

mod error;
mod event;
mod handshake;

pub use {error::Error, event::BondEvent};

/// Handle to a bond with another peer in a group.
pub struct BondHandle {
	events_rx: UnboundedReceiver<BondEvent>,
	commands_tx: UnboundedSender<Command>,
}

impl BondHandle {
	/// Closes the bond connection to the remote peer with the provided
	/// application-level close reason.
	pub async fn close(self, reason: impl CloseReason) {
		let (resp_tx, resp_rx) = oneshot::channel();
		let _ = self
			.commands_tx
			.send(Command::Close(reason.into(), resp_tx));
		let _ = resp_rx.await;
	}
}

impl Stream for BondHandle {
	type Item = BondEvent;

	fn poll_next(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Self::Item>> {
		self.get_mut().events_rx.poll_recv(cx)
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
/// - Bonds use frequent heartbeats to monitor the health and liveness of the
///   connection.
pub struct Bond {
	/// Reference to the shared group state managing this bond.
	group: Arc<GroupState>,

	peer: PeerEntry,

	/// Channel for sending bond events for the group managing this bond.
	events_tx: UnboundedSender<BondEvent>,

	/// Channel for receiving commands to control the bond by the handle.
	commands_rx: UnboundedReceiver<Command>,

	/// The underlying link representing the connection to the remote peer.
	link: Link<Groups>,

	cancel: CancellationToken,
}

impl Bond {
	pub fn spawn(
		group: Arc<GroupState>,
		peer: PeerEntry,
		link: Link<Groups>,
	) -> BondHandle {
		let mut link = link;
		let cancel = group.cancel.child_token();
		let (events_tx, events_rx) = unbounded_channel();
		let (commands_tx, commands_rx) = unbounded_channel();
		link.replace_cancel_token(cancel.clone());

		let bond = Bond {
			group,
			peer,
			events_tx,
			link,
			cancel: cancel.clone(),
			commands_rx,
		};

		tokio::spawn(bond.run());

		BondHandle {
			events_rx,
			commands_tx,
		}
	}
}

impl Bond {
	/// Main loop for managing the bond connection.
	async fn run(mut self) {
		// loop {
		tokio::select! {
			_ = self.cancel.cancelled() => {
				tracing::trace!(
					network = %self.group.local.network_id(),
					peer = %Short(self.peer.id()),
					group = %Short(self.group.key.id()),
					"bond connection cancelled",
				);
			}

			Some(cmd) = self.commands_rx.recv() => {
				match cmd {
					Command::Close(reason, resp_tx) => {
						self.link.close(reason).await.ok();
						let _ = resp_tx.send(());
					}
				}
			}
		}
		//}
	}
}

enum Command {
	Close(ApplicationClose, oneshot::Sender<()>),
}
