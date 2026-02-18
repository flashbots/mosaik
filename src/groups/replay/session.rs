use {
	super::{Config, LogReplaySync, LogReplaySyncMessage},
	crate::{
		PeerId,
		groups::{
			Cursor,
			Index,
			StateMachine,
			StateSyncContext,
			StateSyncSession,
			Term,
		},
		primitives::{AsyncWorkQueue, Pretty, Short, UnboundedChannel},
	},
	core::{
		ops::RangeInclusive,
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
pub struct LogReplaySession<M: StateMachine> {
	/// Must match on all peers of the group.
	config: Config,

	/// Leader `PeerId`, deprioritized for fetches.
	leader: PeerId,

	/// The range of log indices that the follower is missing compared to the
	/// leader. Gets updated as the follower makes progress. Catchup completes
	/// when this range becomes empty.
	gap: RangeInclusive<Index>,

	/// A buffer of log entries received from the leader while the follower is
	/// offline and catching up. These entries will be applied to the state
	/// machine once the follower has fetched any missing entries from peers and
	/// is in sync with the leader.
	buffered: Vec<(Index, Term, M::Command)>,

	/// Used to receive peer termination events from bonds so we can remove them
	/// from availability tracking.
	terminations: UnboundedChannel<PeerId>,

	// Snapshot of bond peer ids from last poll, used to detect new peers joining
	// the group while we're catching up.
	known_bonds: HashSet<PeerId>,

	/// Per-peer availability: which range each peer can serve.
	/// Keyed by `PeerId` for O(1) removal on departure.
	availability: HashMap<PeerId, RangeInclusive<Index>>,

	/// At most one in-flight request per peer.
	inflight: HashMap<PeerId, PendingFetch>,

	/// Out-of-order fetched chunks, keyed by range start.
	/// Non-overlapping because we assign non-overlapping chunks.
	#[allow(clippy::type_complexity)]
	fetched: BTreeMap<Index, (Index, PeerId, Vec<(M::Command, Term)>)>,

	// pending background tasks
	tasks: AsyncWorkQueue,

	/// Wakers for `poll_next_tick` calls that are waiting for the catch-up
	/// process to make some progress.
	wakers: Vec<Waker>,

	// debug metrics
	total: usize,

	// debug metrics
	downloaded: usize,

	// debug metrics
	started_at: Instant,

	// debug metrics
	unique_peers: HashSet<PeerId>,
}

impl<M> core::fmt::Debug for LogReplaySession<M>
where
	M: StateMachine,
{
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		f.debug_struct("LogReplaySession")
			.field("gap", &self.gap)
			.finish_non_exhaustive()
	}
}

impl<M: StateMachine> LogReplaySession<M> {
	pub(super) fn new(
		config: &Config,
		cx: &dyn StateSyncContext<LogReplaySync<M>>,
		position: Cursor,
		entries: Vec<(M::Command, Term)>,
	) -> Self {
		let local_pos = cx.log().last().unwrap_or_default();
		let gap = local_pos.index().next()..=position.index();
		let total = (position.index() - local_pos.index()).as_usize();

		assert_ne!(
			total, 0,
			"no need to create a sync session if we're not behind"
		);

		let leader = cx
			.leader()
			.expect("always known when creating a new sync session; qed");

		let tasks = AsyncWorkQueue::default();
		let terminations = UnboundedChannel::default();

		// watch dropped bonds
		for bond in cx.bonds().iter() {
			let tx = terminations.sender().clone();
			tasks.enqueue(async move {
				bond.terminated().await;
				let _ = tx.send(*bond.peer().id());
			});
		}

		// wake the scheduler on new changes to the bonds set
		let watcher = cx.bonds();
		tasks.enqueue(async move {
			loop {
				// just wake the scheduler, the next `poll_next_tick` will detect
				// the changes and update the known bonds and availability sets.
				watcher.changed().await;
			}
		});

		let pos = position.index().next();

		Self {
			gap,
			leader,
			tasks,
			terminations,
			config: config.clone(),
			buffered: entries
				.into_iter()
				.enumerate()
				.map(|(i, (cmd, term))| (pos + i, term, cmd))
				.collect(),
			known_bonds: HashSet::new(),
			availability: HashMap::new(),
			inflight: HashMap::new(),
			fetched: BTreeMap::new(),
			wakers: Vec::new(),
			downloaded: 0,
			started_at: Instant::now(),
			unique_peers: HashSet::new(),
			total,
		}
	}

	/// Triggers a new `poll_next_tick` call by the group worker.
	fn wake_all(&mut self) {
		for waker in self.wakers.drain(..) {
			waker.wake();
		}
	}

	/// called on every tick to check if any peers have departed and need to be
	/// removed from availability set.
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
				self.availability.remove(&peer);
				self.inflight.remove(&peer);
				self.known_bonds.remove(&peer);
				self.wake_all();
			}
		}
	}

	/// called on every tick to check if any in-flight fetch requests have timed
	/// out and need to be retried or its peer removed.
	fn poll_timeouts(
		&mut self,
		poll_cx: &mut Context<'_>,
		sync_cx: &mut dyn StateSyncContext<LogReplaySync<M>>,
	) {
		let mut timed_out = Vec::new();

		for (peer, pending) in &mut self.inflight {
			if pending.timeout_fut.as_mut().poll(poll_cx).is_ready() {
				timed_out.push((*peer, pending.range.clone()));
			}
		}

		for (peer, range) in timed_out {
			tracing::warn!(
					peer = %Short(peer),
					range = %Pretty(&range),
					group = %Short(sync_cx.group_id()),
					network = %Short(sync_cx.network_id()),
					"fetch request timed out"
			);

			self.inflight.remove(&peer);
			self.availability.remove(&peer);

			// The peer might still be alive but slow. Send a fresh
			// DiscoveryRequest — if it responds, it'll be re-added
			// to availability via add_availability().
			sync_cx.send_to(peer, LogReplaySyncMessage::AvailabilityRequest);

			self.wake_all();
		}
	}

	/// Checks for new peers that have joined the group since the last tick.
	/// Sends availability requests to any new peers and updates the known bonds
	/// set.
	fn poll_new_bonds(
		&mut self,
		cx: &mut dyn StateSyncContext<LogReplaySync<M>>,
	) {
		let current: HashSet<PeerId> =
			cx.bonds().iter().map(|b| *b.peer().id()).collect();

		let new_peers: Vec<PeerId> =
			current.difference(&self.known_bonds).copied().collect();

		for peer in &new_peers {
			cx.send_to(*peer, LogReplaySyncMessage::AvailabilityRequest);
		}

		self.known_bonds = current;
	}

	/// Returns idle peers (no in-flight request), non-leader first.
	fn idle_peers_sorted(&self) -> Vec<PeerId> {
		let mut peers: Vec<PeerId> = self
			.availability
			.keys()
			.filter(|p| !self.inflight.contains_key(*p))
			.copied()
			.collect();

		// Put leader last
		peers.sort_by_key(|p| i32::from(*p == self.leader));
		peers
	}

	/// Called on every tick and after every state change.
	///
	/// Schedules new fetch requests to peers if there are idle peers and
	/// unscheduled gaps.
	fn schedule_fetches(
		&mut self,
		sync_cx: &mut dyn StateSyncContext<LogReplaySync<M>>,
	) {
		let chunk_size = self.config.batch_size;

		// Collect ranges already assigned (in-flight + fetched)
		// to find the next unassigned offset within the gap.
		let mut assigned: Vec<RangeInclusive<Index>> = Vec::new();
		for pending in self.inflight.values() {
			assigned.push(pending.range.clone());
		}
		for (&start, &(end, _, _)) in &self.fetched {
			assigned.push(start..=end);
		}
		assigned.sort_by_key(|r| *r.start());

		// Walk through the gap, skipping assigned ranges,
		// to find consecutive unassigned chunks.
		let mut cursor = *self.gap.start();
		let gap_end = *self.gap.end();

		for range in &assigned {
			if cursor > gap_end {
				break;
			}
			if *range.start() <= cursor && *range.end() >= cursor {
				// cursor falls within an assigned range, skip past it
				cursor = range.end().next();
			}
		}

		// Now assign chunks to idle peers starting from `cursor`
		let mut idle_peers = self.idle_peers_sorted();

		while cursor <= gap_end && !idle_peers.is_empty() {
			let chunk_end = (cursor + (chunk_size - 1)).min(gap_end);

			// Find a peer that covers this chunk (even partially)
			if let Some(idx) = idle_peers.iter().position(|p| {
				self
					.availability
					.get(p)
					.is_some_and(|a| *a.start() <= cursor && *a.end() >= cursor)
			}) {
				let peer = idle_peers.remove(idx);

				// Clamp chunk to what the peer actually has
				let avail = &self.availability[&peer];
				let effective_end = chunk_end.min(*avail.end());
				let effective_chunk = cursor..=effective_end;

				self.send_fetch_request(peer, effective_chunk, sync_cx);
				cursor = effective_end.next();
			} else {
				// No idle peer covers `cursor`, try the next
				// assigned range boundary
				break;
			}

			// Skip over any assigned ranges we may now overlap
			for range in &assigned {
				if *range.start() <= cursor && *range.end() >= cursor {
					cursor = range.end().next();
				}
			}
		}
	}

	fn send_fetch_request(
		&mut self,
		peer: PeerId,
		range: RangeInclusive<Index>,
		sync_cx: &mut dyn StateSyncContext<LogReplaySync<M>>,
	) {
		tracing::trace!(
			peer = %Short(peer),
			range = %Pretty(&range),
			group = %Short(sync_cx.group_id()),
			network = %Short(sync_cx.network_id()),
			"requesting state from"
		);

		sync_cx.send_to(
			peer,
			LogReplaySyncMessage::FetchEntriesRequest(range.clone()),
		);

		let timeout_duration = self.config.fetch_timeout;
		self.inflight.insert(peer, PendingFetch {
			range,
			timeout_fut: Box::pin(sleep(timeout_duration)),
		});
		self.unique_peers.insert(peer);
	}

	/// Called when a peer responds to our `DiscoveryRequest` with a
	/// `AvailabilityResponse` message indicating that it has the missing log
	/// entries available for fetching in the specified range. This method
	/// updates the scheduler's availability tracking and triggers a new
	/// `poll_next_tick` to schedule fetches from the newly available peer.
	fn on_availability_response(
		&mut self,
		peer: PeerId,
		available: RangeInclusive<Index>,
		cx: &dyn StateSyncContext<LogReplaySync<M>>,
	) {
		self.availability.insert(peer, available);

		if let Some(bond) = cx.bonds().get(&peer) {
			// watch for bond termination to remove from availability on departure
			let tx = self.terminations.sender().clone();
			self.tasks.enqueue(async move {
				bond.terminated().await;
				let _ = tx.send(peer);
			});
		}

		self.wake_all();
	}

	/// Called when we receive a batch of log entries from a peer in response to
	/// our `FetchEntriesRequest`.
	fn on_fetch_entries_response(
		&mut self,
		peer: PeerId,
		range: RangeInclusive<Index>,
		entries: Vec<(M::Command, Term)>,
	) {
		// 1. Clear in-flight for this peer (now idle for re-scheduling)
		self.inflight.remove(&peer);

		// 2. Store the fetched chunk
		self
			.fetched
			.insert(*range.start(), (*range.end(), peer, entries));

		// 3. Wakers will trigger next tick to schedule more fetches
		self.wake_all();
	}

	/// Processes all fetched chunks that are contiguous to the current gap start,
	/// appending them to the log and narrowing the gap accordingly. This is
	/// called on every tick to make progress on the sync as fetched chunks
	/// arrive, and also after every state change to process any newly contiguous
	/// chunks. If the gap becomes empty after processing, this also wakes the
	/// scheduler to finalize the sync.
	fn drain_fetched_entries(
		&mut self,
		poll_cx: &Context<'_>,
		sync_cx: &mut dyn StateSyncContext<LogReplaySync<M>>,
	) {
		let mut cursor = *self.gap.start();

		while let Some(entry) = self.fetched.first_key_value() {
			let (&start, _) = entry;
			if start != cursor {
				break; // gap — not contiguous yet
			}
			let (_, (end, peer, entries)) =
				self.fetched.remove_entry(&start).unwrap();

			assert_eq!(
				start,
				sync_cx.log().last().unwrap_or_default().index().next()
			);

			self.downloaded += entries.len();

			if !entries.is_empty() {
				self.wake_all();
			}

			for (command, term) in entries {
				sync_cx.log_mut().append(command, term);
			}

			let synced_range =
				start..=sync_cx.log().last().unwrap_or_default().index();

			let progress = self.downloaded as f64 / self.total as f64 * 100.0;

			tracing::debug!(
				range = %Pretty(&synced_range),
				from = %Short(peer),
				group = %Short(sync_cx.group_id()),
				network = %Short(sync_cx.network_id()),
				"syncing state {progress:.1}% complete",
			);

			cursor = end.next();
		}

		// Narrow the gap
		if cursor != *self.gap.start() {
			self.gap = cursor..=*self.gap.end();
		}

		if self.gap.is_empty() {
			// Sync complete! Buffered entries can now be applied.
			poll_cx.waker().wake_by_ref();
		}
	}

	/// Called when the gap has been fully filled with fetched entries and the
	/// follower is now in sync with the leader. This applies any buffered entries
	/// received during the catch-up process and finalizes the sync session.
	fn finalize_sync(&mut self, cx: &mut dyn StateSyncContext<LogReplaySync<M>>) {
		let mut pos = self.gap.end().next();

		if !self.buffered.is_empty() {
			tracing::trace!(
				pos = %pos,
				count = self.buffered.len(),
				group = %Short(cx.group_id()),
				network = %Short(cx.network_id()),
				"applying buffered state"
			);

			for (index, term, command) in self.buffered.drain(..) {
				// buffered entries are guaranteed to be contiguous and immediately
				// after the last fetched entry, so we can just append them
				// directly.
				assert_eq!(index, pos);
				cx.log_mut().append(command, term);
				pos = pos.next();
			}
		}
	}
}

impl<M: StateMachine> StateSyncSession for LogReplaySession<M> {
	type Owner = LogReplaySync<M>;

	fn poll_next_tick(
		&mut self,
		poll_cx: &mut Context<'_>,
		sync_cx: &mut dyn StateSyncContext<LogReplaySync<M>>,
	) -> Poll<Cursor> {
		self.wakers.push(poll_cx.waker().clone());

		if self.gap.is_empty() {
			// Sync complete! Apply buffered entries and finalize.
			self.finalize_sync(sync_cx);
			let final_pos = sync_cx.log().last().unwrap_or_default();
			tracing::debug!(
				group = %Short(sync_cx.group_id()),
				network = %Short(sync_cx.network_id()),
				"synced {} entries in {:?} from {} peers",
				self.downloaded,
				self.started_at.elapsed(),
				self.unique_peers.len(),
			);
			return Poll::Ready(final_pos);
		}

		// 0. drive background tasks
		let _ = self.tasks.poll_next_unpin(poll_cx);

		// 1. poll bond terminations
		self.poll_terminations(poll_cx);

		// 2. drain any fetched entries that have become contiguous to narrow the
		//    sync gap
		self.drain_fetched_entries(poll_cx, sync_cx);

		// 3. poll in-flight request timeouts
		self.poll_timeouts(poll_cx, sync_cx);

		// 4. poll new bonds
		self.poll_new_bonds(sync_cx);

		// 5. schedule new fetches to fill the pipeline
		self.schedule_fetches(sync_cx);

		Poll::Pending
	}

	/// Receives a sync-related message from a remote peer.
	fn receive(
		&mut self,
		message: LogReplaySyncMessage<M::Command>,
		sender: crate::PeerId,
		cx: &mut dyn StateSyncContext<LogReplaySync<M>>,
	) {
		match message {
			LogReplaySyncMessage::AvailabilityResponse(range) => {
				self.on_availability_response(sender, range, cx);
			}
			LogReplaySyncMessage::FetchEntriesResponse { range, entries } => {
				self.on_fetch_entries_response(sender, range, entries);
			}
			_ => {} // ignore
		}
	}

	/// Accepts a batch of log entries that have been produced by the current
	/// leader while the sync process is ongoing.
	fn buffer(
		&mut self,
		position: Cursor,
		entries: Vec<(M::Command, Term)>,
		_: &mut dyn StateSyncContext<LogReplaySync<M>>,
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

#[derive(Debug)]
struct PendingFetch {
	range: RangeInclusive<Index>,
	timeout_fut: Pin<Box<Sleep>>,
}
