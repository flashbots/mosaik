use {
	super::{Config, SnapshotStateMachine, SnapshotSync, SnapshotSyncMessage},
	crate::{
		PeerId,
		groups::{
			Command,
			Cursor,
			Index,
			Message,
			StateMachine,
			StateSync,
			StateSyncContext,
			StateSyncProvider,
			StateSyncSession,
			Term,
		},
	},
	core::{
		marker::PhantomData,
		task::{Context, Poll},
	},
	std::collections::VecDeque,
};

pub struct SnapshotSyncProvider<M: SnapshotStateMachine> {
	config: Config,
	snapshots: VecDeque<(Index, M::Snapshot)>,
}

impl<M: SnapshotStateMachine> SnapshotSyncProvider<M> {
	pub(super) const fn new(
		config: Config,
		_cx: &dyn StateSyncContext<SnapshotSync<M>>,
	) -> Self {
		Self {
			config,
			snapshots: VecDeque::new(),
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
	) -> Result<(), Message<Self::Owner>> {
		todo!()
	}

	fn committed(
		&mut self,
		index: Index,
		cx: &mut dyn StateSyncContext<Self::Owner>,
	) {
		if index.0.is_multiple_of(self.config.snapshot_interval) {
			let snapshot = cx.machine().snapshot();
			self.snapshots.push_back((index, snapshot));
		}

		if self.snapshots.len() > self.config.window_size {
			self.snapshots.pop_front();
		}
	}
}
