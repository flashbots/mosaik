use {
	crate::{Groups, discovery::PeerEntry, groups::Config, network::link::Link},
	std::sync::Arc,
	tokio::sync::{broadcast, mpsc},
	tokio_util::sync::CancellationToken,
};

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
	link: Link<Groups>,
	peer: PeerEntry,
	config: Arc<Config>,
	cancel: CancellationToken,
	events_tx: mpsc::UnboundedSender<BondEvent>,
}

/// Public API
impl Bond {
	/// Initiates a new bond connection to a remote peer in the group.
	///
	/// Notes:
	/// - The remote peer is expected to be present in the local discovery
	///   catalog.
	pub fn new(
		mut link: Link<Groups>,
		peer: PeerEntry,
		config: Arc<Config>,
		cancel: CancellationToken,
	) -> (Self, mpsc::UnboundedReceiver<BondEvent>) {
		let (events_tx, events_rx) = mpsc::unbounded_channel();
		link.replace_cancel_token(cancel.clone());

		(
			Bond {
				link,
				peer,
				config,
				cancel,
				events_tx,
			},
			events_rx,
		)
	}
}

/// Internal API
impl Bond {
	pub const fn peer(&self) -> &PeerEntry {
		&self.peer
	}
}

#[derive(Debug, Clone, Copy)]
pub enum BondEvent {
	Terminated,
}

struct WorkerLoop;

impl WorkerLoop {}
