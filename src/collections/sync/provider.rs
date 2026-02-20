use {
	super::{
		Config,
		SessionId,
		SnapshotStateMachine,
		SnapshotSync,
		SnapshotSyncMessage,
	},
	crate::{PeerId, collections::sync::Snapshot, groups::*},
	core::{
		marker::PhantomData,
		task::{Context, Poll},
	},
	std::collections::{HashMap, VecDeque},
	tokio::sync::{broadcast, mpsc::UnboundedReceiver},
};

pub struct SnapshotSyncProvider<M: SnapshotStateMachine> {
	config: Config,
	snapshots: HashMap<SessionId, M::Snapshot>,
	snapshots_rx: broadcast::Receiver<(SessionId, M::Snapshot)>,
}

impl<M: SnapshotStateMachine> SnapshotSyncProvider<M> {
	pub(super) fn new(
		config: Config,
		snapshots_rx: broadcast::Receiver<(SessionId, M::Snapshot)>,
		cx: &dyn StateSyncContext<<Self as StateSyncProvider>::Owner>,
	) -> Self {
		let snapshots = HashMap::new();
		Self {
			config,
			snapshots,
			snapshots_rx,
		}
	}
}

impl<M: SnapshotStateMachine> StateSyncProvider for SnapshotSyncProvider<M> {
	type Owner = SnapshotSync<M>;

	fn receive(
		&mut self,
		message: SnapshotSyncMessage,
		sender: PeerId,
		cx: &mut dyn StateSyncContext<Self::Owner>,
	) -> Result<(), SnapshotSyncMessage> {
		match message {
			SnapshotSyncMessage::RequestSnapshot { session_id } => Ok(()),
			_ => Err(message),
		}
	}
}
