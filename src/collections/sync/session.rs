use {
	super::{
		Config,
		Snapshot,
		SnapshotRequest,
		SnapshotStateMachine,
		SnapshotSync,
		SnapshotSyncMessage,
	},
	crate::{PeerId, groups::*, primitives::UnboundedChannel},
	chrono::Utc,
	core::{
		pin::Pin,
		task::{Context, Poll},
	},
	rand::random,
	std::{
		collections::{HashMap, HashSet},
		time::Instant,
	},
	tokio::time::Sleep,
};

pub struct SnapshotSyncSession<M: SnapshotStateMachine> {
	// must match on all instances of the collection on all peers in the group.
	config: Config,

	/// The anchor position of the snapshot being synchronized, this is the
	/// position at which the snapshot was taken and serves as a reference point
	/// for the synchronization process.
	anchor: Index,

	/// A buffer of log entries received from the leader while the follower is
	/// offline and catching up. These entries will be applied to the state
	/// machine once the follower has fetched any missing entries from peers and
	/// is in sync with the leader.
	///
	/// If the snapshot that is being synchronized is past the position of some
	/// of these buffered entries, those entries will be discarded as they are
	/// already included in the snapshot.
	buffered: Vec<(Index, Term, M::Command)>,

	/// Used to receive peer termination events from bonds so we can remove them
	/// from availability tracking.
	terminations: UnboundedChannel<PeerId>,

	/// at most one pending fetch request to a peer at a time.
	inflight: HashMap<PeerId, PendingFetch>,

	/// The snapshot being accumulated from multiple peers during the catch-up
	/// process. The `append` method of the `Snapshot` trait is used to add items
	/// to this snapshot as they are received from peers in chunks. The session
	/// guarantees that the chunks are appended in the correct order and there
	/// are no gaps between them, so the implementation of the `append` method
	/// can simply add the new items to the end of the snapshot.
	accumulated: M::Snapshot,

	// debug metrics
	total: usize,
	downloaded: usize,
	started_at: Instant,
	unique_peers: HashSet<PeerId>,
}

impl<M: SnapshotStateMachine> SnapshotSyncSession<M> {
	pub(super) fn new(
		config: &Config,
		cx: &mut dyn SyncSessionContext<SnapshotSync<M>>,
		position: Cursor,
		leader_commit: Index,
		entries: Vec<(M::Command, Term)>,
	) -> Self {
		// request a new sync snapshot from the leader, this will trigger the leader
		// to append a sync request to the state machine log and make the snapshot
		// available from all in-sync followers.
		cx.send_to(cx.leader(), SnapshotSyncMessage::RequestSnapshot);

		Self {
			total: 0,
			downloaded: 0,
			anchor: position.index(),
			config: config.clone(),
			accumulated: M::Snapshot::default(),
			terminations: UnboundedChannel::default(),
			inflight: HashMap::new(),
			unique_peers: HashSet::new(),
			started_at: Instant::now(),
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

	fn poll(
		&mut self,
		cx: &mut Context<'_>,
		driver: &mut dyn SyncSessionContext<Self::Owner>,
	) -> Poll<Cursor> {
		Poll::Pending
	}

	/// As a session only `SnapshotReady` and `FetchDataResponse` messages are
	/// expected, other message types are handled at the `SyncProvider` level and
	/// should not reach the session.
	fn receive(
		&mut self,
		message: SnapshotSyncMessage<<M::Snapshot as Snapshot>::Item>,
		sender: PeerId,
		cx: &mut dyn SyncSessionContext<Self::Owner>,
	) {
		match message {
			SnapshotSyncMessage::SnapshotReady(info) => {
				tracing::info!(
					">--> received snapshot info from {sender}, size: {} at pos {}",
					info.len,
					self.anchor
				);
			}
			SnapshotSyncMessage::FetchDataResponse(response) => {
				tracing::info!(
					">--> received snapshot data response from {sender}, range: {:?}",
					response.range
				);
			}
			_ => unreachable!("handled at the provider level"),
		}
	}

	fn buffer(
		&mut self,
		position: Cursor,
		entries: Vec<(M::Command, Term)>,
		cx: &mut dyn SyncSessionContext<Self::Owner>,
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

struct PendingFetch {
	range: IndexRange,
	timeout: Pin<Box<Sleep>>,
}
