use {
	super::{
		Config,
		SnapshotRequest,
		SnapshotStateMachine,
		SnapshotSync,
		SnapshotSyncProvider,
		SnapshotSyncSession,
		SyncInitCommand,
	},
	crate::{
		PeerId,
		UniqueId,
		groups::{Cursor, Index, SyncContext, SyncSessionContext, Term},
		primitives::UnboundedChannel,
	},
	chrono::Utc,
	core::{
		any::type_name,
		pin::Pin,
		task::{Context, Poll},
		time::Duration,
	},
	dashmap::{DashMap, Entry},
	std::sync::Arc,
	tokio::{
		sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
		time::{Instant, Sleep, sleep},
	},
};

pub(super) struct SnapshotSyncInner<M: SnapshotStateMachine> {
	config: Config,
	to_command: SyncInitCommand<M>,
	requests_tx: UnboundedSender<(SnapshotRequest, Index, M::Snapshot)>,
	requests_rx: Option<UnboundedReceiver<(SnapshotRequest, Index, M::Snapshot)>>,
}

impl<M: SnapshotStateMachine> SnapshotSyncInner<M> {
	pub fn new(
		config: Config,
		to_command: impl Fn(SnapshotRequest) -> M::Command + Send + Sync + 'static,
	) -> Self {
		let (requests_tx, requests_rx) = unbounded_channel();
		Self {
			config,
			requests_tx,
			requests_rx: Some(requests_rx),
			to_command: Arc::new(to_command),
		}
	}

	pub fn is_expired(&self, request: &SnapshotRequest) -> bool {
		let Ok(elapsed) = Utc::now()
			.signed_duration_since(request.requested_at)
			.abs()
			.to_std()
		else {
			// if the request timestamp is in the future for some reason, consider it
			// expired to be safe.
			return true;
		};

		elapsed > self.config.snapshot_ttl
	}

	pub fn serve_snapshot(
		&self,
		request: SnapshotRequest,
		position: Index,
		snapshot: M::Snapshot,
	) {
		let _ = self.requests_tx.send((request, position, snapshot));
	}
}

// StateSync implementation
impl<M: SnapshotStateMachine> SnapshotSyncInner<M> {
	pub fn signature(&self) -> crate::UniqueId {
		UniqueId::from("mosaik_collections_snapshot_sync")
			.derive(type_name::<M>())
			.derive(self.config.fetch_batch_size.to_le_bytes())
			.derive(self.config.snapshot_ttl.as_millis().to_le_bytes())
	}

	pub fn create_provider(
		&mut self,
		cx: &dyn SyncContext<SnapshotSync<M>>,
	) -> SnapshotSyncProvider<M> {
		let Some(requests_rx) = self.requests_rx.take() else {
			unreachable!(
				"SnapshotSync::create_provider called more than once. this is a bug."
			)
		};

		SnapshotSyncProvider::<M>::new(
			self.config.clone(),
			self.to_command.clone(),
			requests_rx,
			cx,
		)
	}

	pub fn create_session(
		&self,
		cx: &mut dyn SyncSessionContext<SnapshotSync<M>>,
		position: Cursor,
		leader_commit: Index,
		entries: Vec<(M::Command, Term)>,
	) -> SnapshotSyncSession<M> {
		SnapshotSyncSession::new(&self.config, cx, position, leader_commit, entries)
	}
}
