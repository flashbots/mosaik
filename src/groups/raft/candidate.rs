use {
	super::protocol::RequestVote,
	crate::{
		PeerId,
		groups::{
			log::{StateMachine, Storage, Term},
			raft::{Message, leader::Leader, role::Role, shared::Shared},
		},
		primitives::Short,
	},
	core::{
		iter::once,
		marker::PhantomData,
		ops::ControlFlow,
		pin::Pin,
		task::{Context, Poll},
	},
	std::collections::HashSet,
	tokio::time::{Sleep, sleep},
};

/// Internal state for the candidate role that is currently running elections
/// for its leadership candidacy.
#[derive(Debug)]
pub struct Candidate<S: Storage<M::Command>, M: StateMachine> {
	/// The current term for this node.
	term: Term,

	/// The set of peers from which votes have been requested.
	requested_from: HashSet<PeerId>,

	/// The set of peers that have granted their vote.
	votes_granted: HashSet<PeerId>,

	/// The election timeout for the current election round. If this timeout
	/// elapses without reaching a quorum, a new election round will be started.
	election_timeout: Pin<Box<Sleep>>,

	/// Wakers for tasks that are waiting for the election result (e.g., to
	/// become leader or step down to follower). These tasks will be woken up
	/// when a quorum is reached or when stepping down to follower state.
	wakers: Vec<std::task::Waker>,

	#[doc(hidden)]
	_phantom: PhantomData<(S, M)>,
}

impl<S: Storage<M::Command>, M: StateMachine> Candidate<S, M> {
	/// Creates a new candidate role for the specified term and starts the
	/// election process by sending `RequestVote` messages to all bonded peers in
	/// the group.
	pub fn new(term: Term, shared: &mut Shared<S, M>) -> Self {
		assert_ne!(term, 0, "Candidate role should be at least in term 1");

		let election_timeout = shared.config().intervals().election_timeout();
		let election_timeout = Box::pin(sleep(election_timeout));

		let candidate = shared.group().local_id();
		let (last_log_index, last_log_term) = shared.log().last();

		let request = RequestVote {
			term,
			candidate,
			last_log_index,
			last_log_term,
		};

		tracing::debug!(
			term = term,
			group = %Short(shared.group().group_id()),
			network = %Short(shared.group().network_id()),
			"starting new leader election",
		);

		// Broadcast the `RequestVote` message to all bonded peers in the group.
		let requested_from =
			shared.group().bonds.broadcast_raft_message::<M>(request);

		let requested_from =
			requested_from.into_iter().chain(once(candidate)).collect();

		// We start with granting our own vote to ourselves.
		let votes_granted = once(candidate).collect();
		shared.cast_vote(term, candidate);

		Self {
			term,
			requested_from,
			votes_granted,
			election_timeout,
			wakers: Vec::new(),
			_phantom: PhantomData,
		}
	}
}

impl<S: Storage<M::Command>, M: StateMachine> Candidate<S, M> {
	/// As a candidate, we start elections and wait for votes from other nodes or
	/// `AppendEntries` from a leader. If no quorum is reached within the election
	/// timeout, we start a new election.
	///
	/// Returns `Poll::Ready(ControlFlow::Break(new_role))` if the role should
	/// transition to a new state (e.g., follower or leader) or `Poll::Pending`
	/// if it should continue waiting in the candidate state.
	pub fn poll_next_tick(
		&mut self,
		cx: &mut Context,
		shared: &mut Shared<S, M>,
	) -> Poll<ControlFlow<Role<S, M>>> {
		if self.has_quorum() {
			tracing::debug!(
				term = self.term(),
				group = %Short(shared.group().group_id()),
				network = %Short(shared.group().network_id()),
				"received quorum [{}/{}] of votes, becoming leader",
				self.votes_granted.len(),
				self.requested_from.len(),
			);

			return Poll::Ready(ControlFlow::Break(
				Leader::new(self.term, shared).into(),
			));
		}

		if self.election_timeout.as_mut().poll(cx).is_ready() {
			// election timeout elapsed without reaching a quorum, so we start a new
			// election by incrementing the term and broadcasting new RequestVote
			// message to all bonded peers in the group.
			tracing::debug!(
				term = self.term(),
				group = %Short(shared.group().group_id()),
				network = %Short(shared.group().network_id()),
				"election timeout elapsed without reaching quorum, starting new election",
			);

			return Poll::Ready(ControlFlow::Break(
				Self::new(self.term + 1, shared).into(),
			));
		}

		// register the waker for this task so that it can be woken up when we
		// receive enough votes to become leader.
		self.wakers.push(cx.waker().clone());

		Poll::Pending
	}

	/// The current term of this elections round.
	pub const fn term(&self) -> Term {
		self.term
	}

	/// When in a candidate state, we only expect to receive `RequestVoteResponse`
	/// messages from other nodes.
	pub fn receive(
		&mut self,
		message: Message<M::Command>,
		sender: PeerId,
		shared: &Shared<S, M>,
	) {
		let Message::RequestVoteResponse(response) = message else {
			tracing::warn!(
				peer = %Short(sender),
				group = %Short(shared.group().group_id()),
				network = %Short(shared.group().network_id()),
				message = ?message,
				"unexpected message type received in candidate state",
			);
			return;
		};

		if response.vote_granted && self.grant(sender) {
			// Wake up all tasks waiting for the election result
			// since we have reached a quorum and will become the leader.
			for waker in self.wakers.drain(..) {
				waker.wake();
			}
		}
	}

	/// Checks if we have received votes from a quorum of peers to win the
	/// leader election.
	fn has_quorum(&self) -> bool {
		let total_nodes = self.requested_from.len();
		let votes = self.votes_granted.len();
		let quorum = (total_nodes / 2) + 1;
		votes >= quorum
	}

	/// Registers a vote granted by the specified peer and returns `true` if a
	/// quorum has been reached.
	fn grant(&mut self, peer_id: PeerId) -> bool {
		if self.requested_from.contains(&peer_id) {
			// Only count votes from peers we have requested votes from
			self.votes_granted.insert(peer_id);
		}
		self.has_quorum()
	}
}
