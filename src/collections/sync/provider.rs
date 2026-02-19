use {
	super::{Config, SnapshotStateMachine, SnapshotSync, SnapshotSyncMessage},
	crate::{PeerId, groups::*},
	core::{
		marker::PhantomData,
		task::{Context, Poll},
	},
	std::collections::VecDeque,
};

pub struct SnapshotSyncProvider<M: SnapshotStateMachine> {
	config: Config,
	snapshots: VecDeque<M::Snapshot>,
}

impl<M: SnapshotStateMachine> SnapshotSyncProvider<M> {
	pub(super) fn new(
		config: Config,
		cx: &dyn StateSyncContext<<Self as StateSyncProvider>::Owner>,
	) -> Self {
		let mut snapshots = VecDeque::new();
		snapshots.push_back(cx.machine().snapshot(cx));

		Self { config, snapshots }
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
		todo!("handle incoming snapshot sync message")
	}

	/// Invoked when a the committed log index of the group changes.
	fn committed(
		&mut self,
		index: Index,
		cx: &mut dyn StateSyncContext<Self::Owner>,
	) {
		if index.0.is_multiple_of(self.config.interval) {
			self.snapshots.push_back(cx.machine().snapshot(cx));
		}

		if self.snapshots.len() > self.config.retention_window as usize {
			self.snapshots.pop_front();
		}
	}
}
