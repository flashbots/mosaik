use {
	crate::{
		NetworkId,
		PeerId,
		groups::{
			Bonds,
			GroupId,
			Index,
			StateMachine,
			Storage,
			raft::{
				Message,
				protocol::{AppendEntries, LogEntry, Sync},
				shared::Shared,
			},
		},
		primitives::{AsyncWorkQueue, Short, UnboundedChannel},
	},
	core::{
		ops::{ControlFlow, RangeInclusive},
		pin::Pin,
		task::{Context, Poll, Waker},
	},
	futures::StreamExt,
	std::collections::{BTreeMap, HashMap, HashSet},
};

/// Implements the catch-up mechanism for lagging followers in the Raft
/// consensus algorithm.
///
/// Notes:
///
/// - Peers only offer committed log entries to followers catching up with the
///   leader.
///
/// - The catch-process has two main functions:
///   - downloading missing historical log entries from peers that have them
///     available through `Message::Sync(_)` messages that are not part of the
///     standard raft protocol, and
///   - buffering incoming log entries received from the leader while the
///     follower is offline and catching up, coming through raft `AppendEntries`
///     messages.
#[derive(Debug)]
pub struct Catchup<M: StateMachine> {
	/// A buffer of log entries received from the leader while the follower is
	/// offline and catching up. These entries will be applied to the state
	/// machine once the follower has fetched any missing entries from peers and
	/// is in sync with the leader.
	buffered: Vec<LogEntry<M::Command>>,

	/// Responsible for assigning and tracking fetch requests to peers that have
	/// advertised availability of the missing log entries.
	scheduler: Scheduler<M>,

	/// Used in logging when we don't have access to the shared state.
	ids: (GroupId, NetworkId),
}

impl<M: StateMachine> Catchup<M> {
	/// Creates a new `Catchup` instance that manages the catch-up process for a
	/// lagging follower.
	///
	/// This constructor is called only once when the follower first detects that
	/// it is lagging behind the leader. The `position` parameter indicates the
	/// log position of the leader's `AppendEntries` message that triggered the
	/// catch-up process.
	pub fn new<S: Storage<M::Command>>(
		request: AppendEntries<M::Command>,
		shared: &Shared<S, M>,
	) -> Self {
		let gap =
			shared.log.last().index().next()..=request.prev_log_position.index();

		Self {
			buffered: request.entries,
			scheduler: Scheduler::new(gap, request.leader, shared),
			ids: (*shared.group_id(), *shared.network_id()),
		}
	}

	/// Called when we receive a `DiscoveryResponse` message from a peer in
	/// response to our `DiscoveryRequest` message. This method records the range
	/// of log entries that group peers have available for replay.
	pub fn receive_availability(
		&mut self,
		available: RangeInclusive<Index>,
		peer: PeerId,
	) {
		tracing::trace!(
			peer = %Short(peer),
			range = ?available,
			group = %Short(self.ids.0),
			network = %Short(self.ids.1),
			"log availability confirmed from"
		);

		self.scheduler.add_availability(peer, available);
	}

	/// Called when we receive a `FetchEntriesResponse` message from a peer in
	/// response to our `FetchEntriesRequest` message.
	pub fn receive_entries(
		&mut self,
		sender: PeerId,
		range: RangeInclusive<Index>,
		entries: Vec<LogEntry<M::Command>>,
	) {
		self.scheduler.add_entries(sender, range, entries);
	}

	/// Buffers the incoming log entries received from the leader while the
	/// follower is offline and catching up. These entries will be applied to the
	/// state machine once the gap is filled.
	pub fn buffer(&mut self, request: AppendEntries<M::Command>) {
		self.buffered.extend(request.entries);
	}
}

impl<M: StateMachine> Catchup<M> {
	/// Polled by the follower's `poll_next_tick` on every tick while the follower
	/// is in catch-up mode. This runs at the Group worker loop rate.
	pub fn poll_next_tick<S: Storage<M::Command>>(
		&mut self,
		cx: &mut Context<'_>,
		shared: &mut Shared<S, M>,
	) -> Poll<ControlFlow<()>> {
		match self.scheduler.poll_next_tick(cx, shared) {
			// All missing entries have been fetched
			Poll::Ready(CatchupEvent::FetchComplete) => {
				// apply buffered entries to the state machine and complete the catch-up
				// process
				Poll::Ready(ControlFlow::Break(()))
			}

			// new batch of entries have been fetched from a peer,
			Poll::Ready(CatchupEvent::FetchedEntries {
				from,
				position,
				entries,
			}) => {
				assert_eq!(position, shared.log.last().index().next());

				for entry in entries {
					shared.log.append(entry.command, entry.term);
				}

				// advance log position in public api observers
				shared.update_log_pos(shared.log.last());

				tracing::debug!(
					range = ?position..=shared.log.last().index(),
					from = %Short(from),
					group = %Short(self.ids.0),
					network = %Short(self.ids.1),
					"fetched entries"
				);

				// apply the fetched entries to the state machine
				Poll::Ready(ControlFlow::Continue(()))
			}
			Poll::Pending => {
				// store the current waker so the scheduler can trigger a new
				// `poll_next_tick` when it makes progress.
				self.scheduler.add_waker(cx.waker().clone());
				Poll::Pending
			}
		}
	}
}

/// Scheduling rules and preferences:
/// - Maintain at most one in-flight fetch request per peer
/// - Prefer fetching from other peers than the current leader.
/// - Fetch entries in batches of [`StateMachine::catchup_chunk_size()`]
/// - prioritize fetching entries that are closer to the beginning of the
///   remaining gap.
#[derive(Debug)]
struct Scheduler<M: StateMachine> {
	/// The range of log indices that the follower is missing compared to the
	/// leader. Gets updated as the follower makes progress. Catchup completes
	/// when this range becomes empty.
	gap: RangeInclusive<Index>,

	/// Leader `PeerId`, deprioritized for fetches.
	leader: PeerId,

	/// Handle to the group's bonds collection, used to:
	/// - detect newly joined peers and send them `DiscoveryRequests`
	/// - send `FetchEntriesRequests`
	/// - get `bond.terminated()` futures to detect departed peers and remove
	///   them from availability tracking.
	bonds: Bonds,

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
	fetched: BTreeMap<Index, (Index, PeerId, Vec<LogEntry<M::Command>>)>,

	// pending background tasks
	tasks: AsyncWorkQueue,

	/// Events emitted by the scheduler to drive the catch-up process.
	events: UnboundedChannel<CatchupEvent<M>>,

	/// Wakers for `poll_next_tick` calls that are waiting for the catch-up
	/// process to make some progress.
	wakers: Vec<Waker>,

	/// Used in logging when we don't have access to the shared state.
	ids: (GroupId, NetworkId),
}

// Components of the `poll_next_tick` logic
impl<M: StateMachine> Scheduler<M> {
	/// Called on every tick and after every state change.
	fn schedule_fetches<S: Storage<M::Command>>(
		&mut self,
		shared: &Shared<S, M>,
	) {
		let chunk_size = shared.log.machine().catchup_chunk_size().get();

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
			let chunk_end = (cursor + (chunk_size - 1).into()).min(gap_end);

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

				self.send_fetch_request(peer, effective_chunk, shared);
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

	/// called on every tick to check if any in-flight fetch requests have timed
	/// out and need to be retried or its peer removed.
	fn poll_timeouts(&mut self, cx: &mut Context<'_>) {
		let mut timed_out = Vec::new();

		for (peer, pending) in &mut self.inflight {
			if pending.timeout_fut.as_mut().poll(cx).is_ready() {
				timed_out.push((*peer, pending.range.clone()));
			}
		}

		for (peer, range) in timed_out {
			tracing::warn!(
					peer = %Short(peer),
					range = ?range,
					group = %Short(self.ids.0),
					network = %Short(self.ids.1),
					"fetch request timed out"
			);

			self.inflight.remove(&peer);
			self.availability.remove(&peer);

			// The peer might still be alive but slow. Send a fresh
			// DiscoveryRequest — if it responds, it'll be re-added
			// to availability via add_availability().
			self
				.bonds
				.send_raft_to::<M>(Message::Sync(Sync::DiscoveryRequest), peer);

			self.wake_all();
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

	/// Checks for new peers that have joined the group since the last tick.
	fn poll_new_bonds(&mut self) {
		let current: HashSet<PeerId> =
			self.bonds.iter().map(|b| *b.peer().id()).collect();

		let new_peers: Vec<PeerId> =
			current.difference(&self.known_bonds).copied().collect();

		for peer in &new_peers {
			self
				.bonds
				.send_raft_to::<M>(Message::Sync(Sync::DiscoveryRequest), *peer);
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

	fn send_fetch_request<S: Storage<M::Command>>(
		&mut self,
		peer: PeerId,
		range: RangeInclusive<Index>,
		shared: &Shared<S, M>,
	) {
		tracing::trace!(
			peer = %Short(peer),
			range = ?range,
			group = %Short(self.ids.0),
			network = %Short(self.ids.1),
			"requesting log entries from"
		);

		self.bonds.send_raft_to::<M>(
			Message::Sync(Sync::FetchEntriesRequest {
				range: range.clone(),
			}),
			peer,
		);

		let timeout_duration = shared.intervals().catchup_chunk_timeout;
		self.inflight.insert(peer, PendingFetch {
			range,
			timeout_fut: Box::pin(tokio::time::sleep(timeout_duration)),
		});
	}
}

// Public API of the `Scheduler` that is used by the `Catchup` struct to drive
// the catch-up process.
impl<M: StateMachine> Scheduler<M> {
	pub fn new<S: Storage<M::Command>>(
		gap: RangeInclusive<Index>,
		leader: PeerId,
		shared: &Shared<S, M>,
	) -> Self {
		let tasks = AsyncWorkQueue::default();
		let terminations = UnboundedChannel::default();

		// watch dropped bonds
		for bond in shared.bonds().iter() {
			let tx = terminations.sender().clone();
			tasks.enqueue(async move {
				bond.terminated().await;
				let _ = tx.send(*bond.peer().id());
			});
		}

		// wake the scheduler on new changes to the bonds set
		let watcher = shared.bonds().clone();
		tasks.enqueue(async move {
			loop {
				// just wake the scheduler, the next `poll_next_tick` will detect
				// the changes and update the known bonds and availability sets.
				watcher.changed().await;
			}
		});

		Self {
			gap,
			leader,
			bonds: shared.bonds().clone(),
			tasks,
			wakers: Vec::new(),
			inflight: HashMap::new(),
			fetched: BTreeMap::new(),
			known_bonds: HashSet::new(),
			availability: HashMap::new(),
			events: UnboundedChannel::default(),
			terminations: UnboundedChannel::default(),
			ids: (*shared.group_id(), *shared.network_id()),
		}
	}

	/// Called every tick by the `Catchup` struct. Runs at the rate of the Group
	/// worker loop.
	pub fn poll_next_tick<S: Storage<M::Command>>(
		&mut self,
		cx: &mut Context<'_>,
		shared: &Shared<S, M>,
	) -> Poll<CatchupEvent<M>> {
		// 0. drive background tasks
		let _ = self.tasks.poll_next_unpin(cx);

		// 1. poll bond terminations
		self.poll_terminations(cx);

		// 2. poll in-flight fetch timeouts
		self.poll_timeouts(cx);

		// 3. check for new bonds
		self.poll_new_bonds();

		// 4. schedule new fetches to fill the pipeline
		self.schedule_fetches(shared);

		// 5. drain any ready events and bubble them up to the `Catchup` struct
		if let Poll::Ready(Some(event)) = self.events.poll_recv(cx) {
			return Poll::Ready(event);
		}

		self.add_waker(cx.waker().clone());
		Poll::Pending
	}

	pub fn add_waker(&mut self, waker: Waker) {
		self.wakers.push(waker);
	}

	/// Called when a peer responds to our `DiscoveryRequest` with a
	/// `DiscoveryResponse` message indicating that it has the missing log
	/// entries available for fetching in the specified range. This method
	/// updates the scheduler's availability tracking and triggers a new
	/// `poll_next_tick` to schedule fetches from the newly available peer.
	pub fn add_availability(
		&mut self,
		peer: PeerId,
		available: RangeInclusive<Index>,
	) {
		self.availability.insert(peer, available);

		if let Some(bond) = self.bonds.get(&peer) {
			// watch for bond termination to remove from availability on departure
			let tx = self.terminations.sender().clone();
			self.tasks.enqueue(async move {
				bond.terminated().await;
				let _ = tx.send(peer);
			});
		}

		self.wake_all();
	}

	pub fn add_entries(
		&mut self,
		peer: PeerId,
		range: RangeInclusive<Index>,
		entries: Vec<LogEntry<<M as StateMachine>::Command>>,
	) {
		// 1. Clear in-flight for this peer (now idle for re-scheduling)
		self.inflight.remove(&peer);

		// 2. Store the fetched chunk
		self
			.fetched
			.insert(*range.start(), (*range.end(), peer, entries));

		// 3. Drain the longest contiguous prefix from gap.start
		self.drain_and_emit();

		// 4. Wakers will trigger schedule_fetches on next poll
		self.wake_all();
	}

	/// Triggers a new `poll_next_tick` call by the group worker.
	fn wake_all(&mut self) {
		for waker in self.wakers.drain(..) {
			waker.wake();
		}
	}

	fn drain_and_emit(&mut self) {
		let mut cursor = *self.gap.start();

		while let Some(entry) = self.fetched.first_key_value() {
			let (&start, _) = entry;
			if start != cursor {
				break; // gap — not contiguous yet
			}
			let (_, (end, peer, entries)) =
				self.fetched.remove_entry(&start).unwrap();

			self.events.send(CatchupEvent::FetchedEntries {
				from: peer,
				position: start,
				entries,
			});

			cursor = end.next();
		}

		// Narrow the gap
		if cursor != *self.gap.start() {
			self.gap = cursor..=*self.gap.end();
		}

		// Check if done
		if self.gap.is_empty() {
			self.events.send(CatchupEvent::FetchComplete);
		}
	}
}

#[derive(Debug)]
enum CatchupEvent<M: StateMachine> {
	FetchedEntries {
		from: PeerId,
		position: Index,
		entries: Vec<LogEntry<<M as StateMachine>::Command>>,
	},
	FetchComplete,
}

#[derive(Debug)]
struct PendingFetch {
	range: RangeInclusive<Index>,
	timeout_fut: Pin<Box<tokio::time::Sleep>>,
}
