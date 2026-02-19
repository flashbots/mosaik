use {
	super::{Config, SnapshotStateMachine, SnapshotSync},
	crate::{PeerId, collections::sync::SnapshotSyncMessage, groups::*},
	core::task::{Context, Poll},
};

pub struct SnapshotSyncSession<M: SnapshotStateMachine> {
	config: Config,
	buffered: Vec<(Index, Term, M::Command)>,
}

impl<M: SnapshotStateMachine> SnapshotSyncSession<M> {
	pub(super) fn new(
		config: &Config,
		cx: &dyn StateSyncContext<SnapshotSync<M>>,
		position: Cursor,
		leader_commit: Index,
		entries: Vec<(M::Command, Term)>,
	) -> Self {
		Self {
			config: config.clone(),
			buffered: entries
				.into_iter()
				.enumerate()
				.map(|(i, (cmd, term))| (position.index() + i as u64, term, cmd))
				.collect(),
		}
	}
}

impl<M: SnapshotStateMachine> StateSyncSession for SnapshotSyncSession<M> {
	type Owner = SnapshotSync<M>;

	fn poll_next_tick(
		&mut self,
		cx: &mut Context<'_>,
		driver: &mut dyn StateSyncContext<Self::Owner>,
	) -> Poll<Cursor> {
		todo!()
	}

	fn receive(
		&mut self,
		message: SnapshotSyncMessage,
		sender: PeerId,
		cx: &mut dyn StateSyncContext<Self::Owner>,
	) {
		todo!()
	}

	fn buffer(
		&mut self,
		position: Cursor,
		entries: Vec<(M::Command, Term)>,
		cx: &mut dyn StateSyncContext<Self::Owner>,
	) {
		let pos = position.index().next();
		self.buffered.extend(
			entries
				.into_iter()
				.enumerate()
				.map(|(i, (cmd, term))| (pos + i as u64, term, cmd)),
		);
	}
}
