use {
	super::{Config, Snapshot, SnapshotStateMachine, SnapshotSync, protocol::*},
	crate::{
		PeerId,
		groups::{Cursor, Index, StateSyncSession, SyncSessionContext, Term},
		primitives::{AsyncWorkQueue, Pretty, Short, UnboundedChannel},
	},
	core::{
		ops::Range,
		pin::Pin,
		task::{Context, Poll, Waker},
	},
	futures::StreamExt,
	std::{
		collections::{BTreeMap, HashMap, HashSet},
		time::Instant,
	},
	tokio::time::{Sleep, sleep},
};

/// An instance of this type is created by the lagging follower for the duration
/// of the catch-up process and terminated once the follower is fully
/// synchronized with the current state of the group.
pub struct SnapshotSyncSession<M: SnapshotStateMachine> {
	/// Must match on all peers of the group.
	config: Config,

	/// Leader `PeerId`, deprioritized for fetches.
	leader: PeerId,

	/// The position at the leader at which the sync session was created. This
	/// is the point in the log where we detected we were behind.
	trigger_position: Index,

	/// The anchor position of the selected snapshot, set once a valid
	/// `SnapshotReady` message is received and selected.
	anchor: Option<Cursor>,

	/// The total number of items in the selected snapshot, used to calculate
	/// the gap and track progress.
	snapshot_len: u64,

	/// The range of snapshot items that are still missing and need to be
	/// fetched. Narrows as contiguous chunks are drained.
	gap: Option<Range<u64>>,

	/// Candidate anchors received from peers via `SnapshotReady` messages,
	/// stored until we select the best one. Maps anchor cursor to snapshot info
	/// and the set of peers that have it.
	anchor_candidates: HashMap<Cursor, (SnapshotInfo, HashSet<PeerId>)>,

	/// A buffer of log entries received from the leader while the follower is
	/// catching up. These entries will be applied to the state machine once the
	/// follower has fetched the snapshot and is in sync with the leader.
	///
	/// If the snapshot anchor is past the position of some of these buffered
	/// entries, those entries will be discarded as they are already included in
	/// the snapshot.
	buffered: Vec<(Index, Term, M::Command)>,

	/// Used to receive peer termination events from bonds so we can remove them
	/// from availability tracking.
	terminations: UnboundedChannel<PeerId>,

	/// Snapshot of bond peer ids from last poll, used to detect new peers.
	known_bonds: HashSet<PeerId>,

	/// Peers that have reported their snapshot is ready and can serve data.
	/// All snapshot-ready peers have the full snapshot (binary availability).
	available_peers: HashSet<PeerId>,

	/// At most one in-flight fetch request per peer.
	inflight: HashMap<PeerId, PendingFetch>,

	/// Out-of-order fetched chunks, keyed by range start.
	/// Non-overlapping because we assign non-overlapping chunks.
	#[allow(clippy::type_complexity)]
	fetched: BTreeMap<u64, (u64, Vec<<M::Snapshot as Snapshot>::Item>)>,

	/// The snapshot being accumulated from multiple peers during the catch-up
	/// process. The `append` method of the `Snapshot` trait is used to add items
	/// to this snapshot as they are received from peers in chunks. The session
	/// guarantees that the chunks are appended in the correct order and there
	/// are no gaps between them.
	accumulated: M::Snapshot,

	/// Timer for retrying `RequestSnapshot` if the leader doesn't respond.
	/// Set to `None` once we receive the first `SnapshotReady`.
	snapshot_request_timer: Option<Pin<Box<Sleep>>>,

	/// Pending background tasks (bond termination watchers, bond change
	/// watcher).
	tasks: AsyncWorkQueue,

	/// Wakers for `poll` calls that are waiting for the catch-up process to
	/// make some progress.
	wakers: Vec<Waker>,

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
		_leader_commit: Index,
		entries: Vec<(M::Command, Term)>,
	) -> Self {
		let tasks = AsyncWorkQueue::default();
		let terminations = UnboundedChannel::default();

		// Watch existing bonds for termination
		for bond in cx.bonds().iter() {
			let tx = terminations.sender().clone();
			tasks.enqueue(async move {
				bond.terminated().await;
				let _ = tx.send(*bond.peer().id());
			});
		}

		// Wake the scheduler on new changes to the bonds set
		let watcher = cx.bonds();
		tasks.enqueue(async move {
			loop {
				watcher.changed().await;
			}
		});

		// Request a snapshot from the leader
		cx.send_to(cx.leader(), SnapshotSyncMessage::RequestSnapshot);

		let pos = position.index().next();

		Self {
			leader: cx.leader(),
			trigger_position: position.index(),
			anchor: None,
			snapshot_len: 0,
			gap: None,
			anchor_candidates: HashMap::new(),
			config: config.clone(),
			accumulated: M::Snapshot::default(),
			terminations,
			known_bonds: HashSet::new(),
			available_peers: HashSet::new(),
			inflight: HashMap::new(),
			fetched: BTreeMap::new(),
			snapshot_request_timer: Some(Box::pin(sleep(
				config.snapshot_request_timeout,
			))),
			tasks,
			wakers: Vec::new(),
			total: 0,
			downloaded: 0,
			started_at: Instant::now(),
			unique_peers: HashSet::new(),
			buffered: entries
				.into_iter()
				.enumerate()
				.map(|(i, (cmd, term))| (pos + i, term, cmd))
				.collect(),
		}
	}

	/// Triggers a new `poll` call by the group worker.
	fn wake_all(&mut self) {
		for waker in self.wakers.drain(..) {
			waker.wake();
		}
	}

	/// Returns the earliest buffered entry index, or `None` if the buffer is
	/// empty.
	fn earliest_buffered(&self) -> Option<Index> {
		self.buffered.first().map(|(idx, _, _)| *idx)
	}

	/// Attempts to select the best anchor from the candidates.
	///
	/// The best anchor is the highest index that is still usable, i.e. where
	/// the snapshot position is >= the earliest buffered entry so there's no
	/// gap between the snapshot end and the buffer start.
	///
	/// If no buffered entries exist, any anchor is valid (the buffer will be
	/// populated later from the leader).
	fn try_select_anchor(
		&mut self,
		cx: &dyn SyncSessionContext<SnapshotSync<M>>,
	) -> bool {
		if self.anchor.is_some() {
			// Already selected — check for upgrade
			return self.try_upgrade_anchor(cx);
		}

		let earliest_buf = self.earliest_buffered();

		// Sort candidates by anchor index descending (highest first)
		let mut candidates: Vec<_> = self.anchor_candidates.iter().collect();
		candidates.sort_by(|a, b| b.0.index().cmp(&a.0.index()));

		for (anchor_cursor, (info, _peers)) in &candidates {
			let valid =
				earliest_buf.is_none_or(|earliest| anchor_cursor.index() >= earliest);

			if valid {
				tracing::info!(
					anchor = %anchor_cursor,
					items = info.items_count,
					group = %Short(cx.group_id()),
					network = %Short(cx.network_id()),
					"selected snapshot anchor"
				);

				self.anchor = Some(**anchor_cursor);
				self.snapshot_len = info.items_count;
				self.total = info.items_count as usize;

				if info.items_count > 0 {
					self.gap = Some(0..info.items_count);
				} else {
					// Empty snapshot — nothing to fetch
					self.gap = Some(0..0);
				}

				// Mark all peers that have this anchor as available
				if let Some((_, peers)) = self.anchor_candidates.get(anchor_cursor) {
					for peer in peers {
						self.available_peers.insert(*peer);
					}
				}

				return true;
			}
		}

		false
	}

	/// Check if a higher anchor has become available and upgrade to it,
	/// discarding any in-progress fetch state. This is safe because all
	/// snapshot-ready peers have the full snapshot.
	fn try_upgrade_anchor(
		&mut self,
		cx: &dyn SyncSessionContext<SnapshotSync<M>>,
	) -> bool {
		let Some(current) = self.anchor else {
			return false;
		};

		let earliest_buf = self.earliest_buffered();

		let best = self
			.anchor_candidates
			.iter()
			.filter(|(cursor, _)| cursor.index() > current.index())
			.filter(|(cursor, _)| {
				earliest_buf.is_none_or(|earliest| cursor.index() >= earliest)
			})
			.max_by_key(|(cursor, _)| cursor.index());

		if let Some((&new_anchor, (info, peers))) = best {
			tracing::info!(
				old_anchor = %current,
				new_anchor = %new_anchor,
				len = info.items_count,
				group = %Short(cx.group_id()),
				network = %Short(cx.network_id()),
				"upgrading snapshot anchor"
			);

			// Reset fetch state
			self.anchor = Some(new_anchor);
			self.snapshot_len = info.items_count;
			self.total = info.items_count as usize;
			self.downloaded = 0;
			self.accumulated = M::Snapshot::default();
			self.inflight.clear();
			self.fetched.clear();

			self.gap = Some(0..info.items_count);

			// Update available peers for the new anchor
			self.available_peers.clear();
			for peer in peers {
				self.available_peers.insert(*peer);
			}

			true
		} else {
			false
		}
	}

	/// Called when a peer reports that it has a snapshot ready at a given
	/// anchor position.
	fn on_snapshot_ready(
		&mut self,
		peer: PeerId,
		info: SnapshotInfo,
		cx: &dyn SyncSessionContext<SnapshotSync<M>>,
	) {
		tracing::debug!(
			from = %Short(peer),
			anchor = %info.anchor,
			items = info.items_count,
			group = %Short(cx.group_id()),
			network = %Short(cx.network_id()),
			"snapshot available"
		);

		// Stop retrying RequestSnapshot after the first SnapshotReady
		self.snapshot_request_timer = None;

		// Record this candidate
		let entry = self
			.anchor_candidates
			.entry(info.anchor)
			.or_insert_with(|| (info, HashSet::new()));
		entry.1.insert(peer);

		// If this peer's anchor matches our currently selected anchor, add them
		// to availability immediately.
		if self.anchor == Some(info.anchor) {
			self.available_peers.insert(peer);
		}

		// Watch for this peer's bond termination if we haven't already
		if let Some(bond) = cx.bonds().get(&peer) {
			let tx = self.terminations.sender().clone();
			self.tasks.enqueue(async move {
				bond.terminated().await;
				let _ = tx.send(peer);
			});
		}

		// Try to select or upgrade the anchor
		self.try_select_anchor(cx);

		self.wake_all();
	}

	/// Called when we receive a batch of snapshot data items from a peer.
	fn on_fetch_data_response(
		&mut self,
		peer: PeerId,
		response: FetchDataResponse<<M::Snapshot as Snapshot>::Item>,
	) {
		// Ignore responses for a different anchor (stale from before
		// an anchor upgrade)
		if self.anchor != Some(response.anchor) {
			return;
		}

		// Clear in-flight for this peer (now idle for re-scheduling)
		self.inflight.remove(&peer);

		let start = response.offset;
		let end = start + response.items.len() as u64;

		// Store the fetched chunk
		self.fetched.insert(start, (end, response.items));

		self.wake_all();
	}

	/// Checks if any peers have departed and removes them from availability.
	fn poll_terminations(&mut self, cx: &mut Context<'_>) {
		if self.terminations.is_empty() {
			return;
		}

		let count = self.terminations.len();
		let mut terminated = Vec::with_capacity(count);
		if self
			.terminations
			.poll_recv_many(cx, &mut terminated, count)
			.is_ready()
		{
			for peer in terminated {
				self.available_peers.remove(&peer);
				self.inflight.remove(&peer);
				self.known_bonds.remove(&peer);
				// Also remove from anchor candidates
				for (_, peers) in self.anchor_candidates.values_mut() {
					peers.remove(&peer);
				}
				self.wake_all();
			}
		}
	}

	/// Checks if any in-flight fetch requests have timed out.
	fn poll_timeouts(
		&mut self,
		poll_cx: &mut Context<'_>,
		sync_cx: &dyn SyncSessionContext<SnapshotSync<M>>,
	) {
		let mut timed_out = Vec::new();

		for (peer, pending) in &mut self.inflight {
			if pending.timeout.as_mut().poll(poll_cx).is_ready() {
				timed_out.push((*peer, pending.range.clone()));
			}
		}

		for (peer, range) in timed_out {
			tracing::warn!(
				peer = %Short(peer),
				range = ?range,
				group = %Short(sync_cx.group_id()),
				network = %Short(sync_cx.network_id()),
				"snapshot fetch request timed out"
			);

			self.inflight.remove(&peer);
			self.available_peers.remove(&peer);
			self.wake_all();
		}
	}

	/// Polls the snapshot request timer. If it fires, resend `RequestSnapshot`
	/// to the current leader and reset the timer.
	fn poll_snapshot_request_timeout(
		&mut self,
		poll_cx: &mut Context<'_>,
		sync_cx: &mut dyn SyncSessionContext<SnapshotSync<M>>,
	) {
		let Some(timer) = &mut self.snapshot_request_timer else {
			return;
		};

		if timer.as_mut().poll(poll_cx).is_ready() {
			tracing::debug!(
				leader = %Short(sync_cx.leader()),
				group = %Short(sync_cx.group_id()),
				network = %Short(sync_cx.network_id()),
				"retrying snapshot request"
			);

			sync_cx.send_to(sync_cx.leader(), SnapshotSyncMessage::RequestSnapshot);

			// Reset timer
			self.snapshot_request_timer =
				Some(Box::pin(sleep(self.config.snapshot_request_timeout)));

			// Re-register the new timer with the waker
			if let Some(t) = &mut self.snapshot_request_timer {
				let _ = t.as_mut().poll(poll_cx);
			}
		}
	}

	/// Checks for new peers that have joined the group since the last tick.
	fn poll_new_bonds(&mut self, cx: &dyn SyncSessionContext<SnapshotSync<M>>) {
		let current: HashSet<PeerId> =
			cx.bonds().iter().map(|b| *b.peer().id()).collect();

		// New peers don't need explicit requests — they announce themselves
		// via SnapshotReady if they have a snapshot for us.
		self.known_bonds = current;
	}

	/// Returns idle peers (no in-flight request), non-leader first.
	fn idle_peers_sorted(&self) -> Vec<PeerId> {
		let mut peers: Vec<PeerId> = self
			.available_peers
			.iter()
			.filter(|p| !self.inflight.contains_key(*p))
			.copied()
			.collect();

		// Put leader last to reduce load on it
		peers.sort_by_key(|p| i32::from(*p == self.leader));
		peers
	}

	/// Schedules new fetch requests to idle peers for unassigned chunks.
	fn schedule_fetches(
		&mut self,
		sync_cx: &mut dyn SyncSessionContext<SnapshotSync<M>>,
	) {
		let gap = match &self.gap {
			Some(g) if !g.is_empty() => g.clone(),
			_ => return,
		};

		let Some(anchor) = self.anchor else { return };

		let chunk_size = self.config.fetch_batch_size;

		// Collect ranges already assigned (in-flight + fetched) to find the
		// next unassigned offset within the gap.
		let mut assigned: Vec<Range<u64>> = Vec::new();
		for pending in self.inflight.values() {
			assigned.push(pending.range.clone());
		}
		for (&start, (end, _)) in &self.fetched {
			assigned.push(start..*end);
		}
		assigned.sort_by_key(|r| r.start);

		// Walk through the gap, skipping assigned ranges
		let mut cursor = gap.start;
		let gap_end = gap.end;

		for range in &assigned {
			if cursor >= gap_end {
				break;
			}
			if range.start <= cursor && range.end > cursor {
				cursor = range.end;
			}
		}

		// Assign chunks to idle peers starting from `cursor`
		let mut idle_peers = self.idle_peers_sorted();

		while cursor < gap_end && !idle_peers.is_empty() {
			let chunk_end = (cursor + chunk_size).min(gap_end);
			let peer = idle_peers.remove(0);

			let effective_chunk = cursor..chunk_end;
			self.send_fetch_request(peer, anchor, effective_chunk, sync_cx);
			cursor = chunk_end;

			// Skip over any assigned ranges we may now overlap
			for range in &assigned {
				if range.start <= cursor && range.end > cursor {
					cursor = range.end;
				}
			}
		}
	}

	fn send_fetch_request(
		&mut self,
		peer: PeerId,
		anchor: Cursor,
		range: Range<u64>,
		sync_cx: &mut dyn SyncSessionContext<SnapshotSync<M>>,
	) {
		tracing::trace!(
			peer = %Short(peer),
			range = ?range,
			anchor = %anchor,
			group = %Short(sync_cx.group_id()),
			network = %Short(sync_cx.network_id()),
			"syncing from"
		);

		sync_cx.send_to(
			peer,
			SnapshotSyncMessage::FetchDataRequest(FetchDataRequest {
				anchor,
				range: range.clone(),
			}),
		);

		let timeout_duration = self.config.fetch_timeout;
		self.inflight.insert(peer, PendingFetch {
			range,
			timeout: Box::pin(sleep(timeout_duration)),
		});
		self.unique_peers.insert(peer);
	}

	/// Processes all fetched chunks that are contiguous to the current gap
	/// start, appending them to the accumulated snapshot and narrowing the
	/// gap accordingly.
	fn drain_fetched_items(
		&mut self,
		poll_cx: &Context<'_>,
		sync_cx: &dyn SyncSessionContext<SnapshotSync<M>>,
	) {
		let gap = match &self.gap {
			Some(g) if !g.is_empty() => g.clone(),
			_ => return,
		};

		let mut cursor = gap.start;

		while let Some(entry) = self.fetched.first_key_value() {
			let (&start, _) = entry;
			if start != cursor {
				break; // gap — not contiguous yet
			}
			let (_, (end, items)) = self.fetched.remove_entry(&start).unwrap();

			self.downloaded += items.len();
			if !items.is_empty() {
				self.wake_all();
			}

			self.accumulated.append(items);

			let progress = self.downloaded as f64 / self.total.max(1) as f64 * 100.0;

			tracing::debug!(
				range = ?(start..end),
				total = self.total,
				downloaded = self.downloaded,
				group = %Short(sync_cx.group_id()),
				network = %Short(sync_cx.network_id()),
				"syncing snapshot {progress:.1}%",
			);

			cursor = end;
		}

		// Narrow the gap
		if cursor != gap.start {
			self.gap = Some(cursor..gap.end);
		}

		if self.gap.as_ref().is_some_and(|g| g.is_empty()) {
			// All snapshot data fetched! Wake to finalize.
			poll_cx.waker().wake_by_ref();
		}
	}

	/// Called when the gap has been fully filled and we're ready to install.
	/// Installs the accumulated snapshot into the state machine, prunes the
	/// log up to the anchor, then appends buffered entries that come after
	/// the anchor.
	fn finalize_sync(
		&mut self,
		cx: &mut dyn SyncSessionContext<SnapshotSync<M>>,
	) {
		let anchor = self.anchor.expect("finalize_sync called without an anchor");

		// Install the snapshot
		let snapshot = core::mem::take(&mut self.accumulated);
		cx.state_machine_mut().install_snapshot(snapshot);

		// Fast-forward the log and committed index to the anchor position so
		// that the raft layer sees the follower as caught up to this point.
		cx.log_mut().reset_to(anchor);
		cx.set_committed(anchor);

		// Discard buffered entries that are at or before the anchor
		// (they're already in the snapshot)
		self.buffered.retain(|(idx, _, _)| *idx > anchor.index());

		// Apply remaining buffered entries
		if !self.buffered.is_empty() {
			let mut pos = anchor.index().next();

			tracing::trace!(
				pos = %pos,
				count = self.buffered.len(),
				group = %Short(cx.group_id()),
				network = %Short(cx.network_id()),
				"applying buffered entries after snapshot"
			);

			for (index, term, command) in self.buffered.drain(..) {
				assert_eq!(index, pos);
				cx.log_mut().append(command, term);
				pos = pos.next();
			}
		}
	}

	/// Returns `true` if all snapshot data has been fetched (gap is empty)
	/// and we have a selected anchor.
	fn is_sync_complete(&self) -> bool {
		self.anchor.is_some() && self.gap.as_ref().is_some_and(|g| g.is_empty())
	}
}

impl<M: SnapshotStateMachine> StateSyncSession for SnapshotSyncSession<M> {
	type Owner = SnapshotSync<M>;

	fn poll(
		&mut self,
		poll_cx: &mut Context<'_>,
		sync_cx: &mut dyn SyncSessionContext<Self::Owner>,
	) -> Poll<Cursor> {
		self.wakers.push(poll_cx.waker().clone());

		if self.is_sync_complete() {
			// All snapshot data fetched — install and finalize.
			self.finalize_sync(sync_cx);

			tracing::debug!(
				group = %Short(sync_cx.group_id()),
				network = %Short(sync_cx.network_id()),
				"snapshot sync completed: {} items in {:?} from {} peers",
				self.downloaded,
				self.started_at.elapsed(),
				self.unique_peers.len(),
			);

			return Poll::Ready(sync_cx.log().last());
		}

		// 0. Drive background tasks (bond termination watchers, etc.)
		let _ = self.tasks.poll_next_unpin(poll_cx);

		// 1. Poll bond terminations
		self.poll_terminations(poll_cx);

		// 2. Poll snapshot request retry timer
		self.poll_snapshot_request_timeout(poll_cx, sync_cx);

		// 3. Try to select or upgrade anchor if we haven't yet
		self.try_select_anchor(sync_cx);

		// 4. Drain any fetched items that have become contiguous
		self.drain_fetched_items(poll_cx, sync_cx);

		// 5. Poll in-flight request timeouts
		self.poll_timeouts(poll_cx, sync_cx);

		// 6. Detect new bonds
		self.poll_new_bonds(sync_cx);

		// 7. Schedule new fetches to fill the pipeline
		self.schedule_fetches(sync_cx);

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
			SnapshotSyncMessage::SnapshotOffer(info) => {
				self.on_snapshot_ready(sender, info, cx);
			}
			SnapshotSyncMessage::FetchDataResponse(response) => {
				self.on_fetch_data_response(sender, response);
			}
			_ => unreachable!("handled at the provider level"),
		}
	}

	fn buffer(
		&mut self,
		position: Cursor,
		entries: Vec<(M::Command, Term)>,
		_cx: &mut dyn SyncSessionContext<Self::Owner>,
	) {
		let pos = position.index().next();
		self.buffered.extend(
			entries
				.into_iter()
				.enumerate()
				.map(|(i, (cmd, term))| (pos + i, term, cmd)),
		);
	}
}

struct PendingFetch {
	range: Range<u64>,
	timeout: Pin<Box<Sleep>>,
}
