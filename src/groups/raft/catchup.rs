use {
	crate::{
		NetworkId,
		PeerId,
		groups::{
			Cursor,
			GroupId,
			Index,
			StateMachine,
			Storage,
			raft::{
				Message,
				protocol::{LogEntry, Sync},
				shared::Shared,
			},
		},
		primitives::Short,
	},
	core::{
		ops::{ControlFlow, RangeInclusive},
		pin::Pin,
		task::{Context, Poll},
	},
	std::{collections::HashMap, time::Instant},
};

/// Implements the catch-up mechanism for lagging followers in the Raft
/// consensus algorithm.
///
/// Notes:
///
/// - Peers only offer committed log entries to followers catching up with the
///   leader.
#[derive(Debug)]
pub struct Catchup<M: StateMachine> {
	/// The range of log indices that the follower is missing compared to the
	/// leader.
	gap: RangeInclusive<Index>,

	/// Keeps track of which peers have responded to our `DiscoveryRequest`
	/// message and the range of log entries they have available for replay.
	available: HashMap<PeerId, RangeInclusive<Index>>,

	/// In-flight fetches of log entries from peers. There should be at most one
	/// pending fetch per peer at any given time. Those peers are the ones that
	/// responded to our `DiscoveryRequest` message and have the relevant log
	/// entries available for replay.
	pending: HashMap<PeerId, PendingFetch>,

	/// A buffer of log entries received from the leader while the follower is
	/// offline and catching up. These entries will be applied to the state
	/// machine once the follower has fetched any missing entries from peers and
	/// is in sync with the leader.
	buffered_entries: Vec<LogEntry<M::Command>>,

	/// Used in logging when we don't have access to the shared state.
	ids: (GroupId, NetworkId),

	/// Wakers for tasks that are waiting for the catch-up process to make some
	/// progress.
	wakers: Vec<std::task::Waker>,
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
		position: Cursor,
		shared: &Shared<S, M>,
	) -> Self {
		let gap = shared.log.last().index() + Index::one()..=position.index();

		// Broadcast a `DiscoveryRequest` message to all bonded peers to discover
		// which peers have the log entries in the missing range so that we can
		// coordinate the catch-up process and distribute the fetching of among
		// them.
		shared
			.bonds()
			.broadcast_raft::<M>(Message::Sync(Sync::DiscoveryRequest));

		Self {
			gap,
			wakers: Vec::new(),
			pending: HashMap::new(),
			available: HashMap::new(),
			buffered_entries: Vec::new(),
			ids: (*shared.group_id(), *shared.network_id()),
		}
	}

	/// Called when we receive a `DiscoveryResponse` message from a peer in
	/// response to our `DiscoveryRequest` message. This method records the range
	/// of log entries that group peers have available for replay.
	pub fn record_availability(
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

		self.available.insert(peer, available);
		self.wake_poll_next_tick();
	}

	/// Called when we receive a `FetchEntriesResponse` message from a peer in
	/// response to our `FetchEntriesRequest` message.
	pub fn receive_entries(
		&mut self,
		sender: PeerId,
		range: RangeInclusive<Index>,
		entries: Vec<LogEntry<M::Command>>,
	) {
		tracing::trace!(
			range = ?range,
			count = entries.len(),
			from = %Short(sender),
			group = %Short(self.ids.0),
			network = %Short(self.ids.1),
			"fetched log entries"
		);

		self.wake_poll_next_tick();
	}

	/// Buffers the incoming log entries received from the leader while the
	/// follower is offline and catching up. These entries will be applied to the
	/// state machine once the gap is filled.
	pub fn buffer_current_entries(
		&mut self,
		_position: Cursor,
		entries: Vec<LogEntry<M::Command>>,
	) {
		self.buffered_entries.extend(entries);
		self.wake_poll_next_tick();
	}
}

impl<M: StateMachine> Catchup<M> {
	/// Polled by the follower's `poll_next_tick` on every tick while the follower
	/// is in catch-up mode. This runs at the Group worker loop rate.
	pub fn poll_next_tick<S: Storage<M::Command>>(
		&mut self,
		cx: &mut Context<'_>,
		shared: &Shared<S, M>,
	) -> Poll<ControlFlow<()>> {
		// remove any pending fetches that have timed out without receiving a
		// response from the peer.
		self.clean_timed_out_fetches(cx, shared);

		tracing::info!(
			leader = ?shared.leader(),
			group = %Short(self.ids.0),
			network = %Short(self.ids.1),
			gap = ?self.gap,
			"polling catch-up progress"
		);

		self.wakers.push(cx.waker().clone());

		Poll::Pending
	}

	/// Walks through the pending fetches and removes any that have timed out
	/// without receiving a response from the peer. This allows the follower to
	/// retry fetching the missing log entries from other peers that have them
	/// available.
	fn clean_timed_out_fetches<S: Storage<M::Command>>(
		&mut self,
		cx: &mut Context,
		shared: &Shared<S, M>,
	) {
		let mut expired = Vec::new();
		for (peer, pending) in &mut self.pending {
			if pending.timeout_fut.as_mut().poll(cx).is_ready() {
				tracing::info!(
					peer = %Short(*peer),
					group = %Short(self.ids.0),
					network = %Short(self.ids.1),
					"fetch request timeout"
				);
				expired.push(*peer);
			}
		}

		for peer in expired {
			// remove the peer that timed out from the available peers list
			self.pending.remove(&peer);
			self.available.remove(&peer);

			// but still resend a new `DiscoveryRequest` to the peer to check if it's
			// still available and has the relevant log entries for catch-up, in case
			// the timeout was just a transient issue. If they respond, they'll be
			// added back to the available list and we can retry fetching from them on
			// the next tick.
			shared
				.bonds()
				.send_raft_to::<M>(Sync::<M::Command>::DiscoveryRequest, peer);
		}
	}

	fn wake_poll_next_tick(&mut self) {
		for waker in self.wakers.drain(..) {
			waker.wake();
		}
	}
}

#[derive(Debug)]
struct PendingFetch {
	range: RangeInclusive<Index>,
	timeout_at: Instant,
	timeout_fut: Pin<Box<tokio::time::Sleep>>,
}
