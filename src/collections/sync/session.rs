use {
	super::{SnapshotStateMachine, SnapshotSync},
	crate::{
		PeerId,
		collections::sync::SnapshotSyncMessage,
		groups::{
			Command,
			Cursor,
			Message,
			StateMachine,
			StateSync,
			StateSyncContext,
			StateSyncProvider,
			StateSyncSession,
			Term,
		},
	},
	core::task::{Context, Poll},
};

pub struct SnapshotSyncSession<M: SnapshotStateMachine> {
	#[doc(hidden)]
	_marker: core::marker::PhantomData<M>,
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
		todo!()
	}
}
