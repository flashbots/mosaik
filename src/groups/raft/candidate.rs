use {
	super::protocol::RequestVote,
	crate::{
		PeerId,
		groups::{
			log::{StateMachine, Storage, Term},
			raft::{
				Message,
				leader::Leader,
				protocol::Vote,
				role::Role,
				shared::Shared,
			},
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
pub struct Candidate<M: StateMachine> {
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
	_phantom: PhantomData<M>,
}

impl<M: StateMachine> Candidate<M> {
	/// Creates a new candidate role for the specified term and starts the
	/// election process by sending `RequestVote` messages to all bonded peers in
	/// the group.
	pub fn new<S: Storage<M::Command>>(
		term: Term,
		shared: &mut Shared<S, M>,
	) -> Self {
		assert_ne!(
			term,
			Term::zero(),
			"Candidate role should be at least in term 1"
		);

		// nodes in candidate state are always considered offline
		shared.set_offline();

		let election_timeout = shared.intervals().election_timeout();
		let election_timeout = Box::pin(sleep(election_timeout));

		let candidate = shared.local_id();
		let log_position = shared.log.last();

		let request = RequestVote {
			term,
			candidate,
			log_position,
		};

		tracing::debug!(
			term = %term,
			log = %log_position,
			group = %Short(shared.group_id()),
			network = %Short(shared.network_id()),
			"starting new leader elections",
		);

		// Broadcast the `RequestVote` message to all bonded peers in the group.
		let requested_from = shared.bonds().broadcast_raft::<M>(request);

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

impl<M: StateMachine> Candidate<M> {
	/// As a candidate, we start elections and wait for votes from other nodes or
	/// `AppendEntries` from a leader. If no quorum is reached within the election
	/// timeout, we start a new election.
	///
	/// Returns `Poll::Ready(ControlFlow::Break(new_role))` if the role should
	/// transition to a new state (e.g., follower or leader) or `Poll::Pending`
	/// if it should continue waiting in the candidate state.
	pub fn poll_next_tick<S: Storage<M::Command>>(
		&mut self,
		cx: &mut Context,
		shared: &mut Shared<S, M>,
	) -> Poll<ControlFlow<Role<M>>> {
		if self.quorum_reached() {
			tracing::debug!(
				term = %self.term(),
				group = %Short(shared.group_id()),
				network = %Short(shared.network_id()),
				"received quorum [{}/{}] of votes, becoming leader",
				self.votes_granted.len(),
				self.requested_from.len(),
			);

			return Poll::Ready(ControlFlow::Break(
				Leader::new(self.term, self.votes_granted.clone(), shared).into(),
			));
		}

		if self.election_timeout.as_mut().poll(cx).is_ready() {
			// election timeout elapsed without reaching a quorum, so we start a new
			// election by incrementing the term and broadcasting new RequestVote
			// message to all bonded peers in the group.
			tracing::debug!(
				term = %self.term(),
				group = %Short(shared.group_id()),
				network = %Short(shared.network_id()),
				committee_size = self.requested_from.len(),
				votes_granted = self.votes_granted.len(),
				"quorum not reached",
			);

			return Poll::Ready(ControlFlow::Break(
				Self::new(self.term.next(), shared).into(),
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
	pub fn receive_protocol_message<S: Storage<M::Command>>(
		&mut self,
		message: Message<M::Command>,
		sender: PeerId,
		shared: &Shared<S, M>,
	) {
		let Message::RequestVoteResponse(response) = message else {
			tracing::trace!(
				peer = %Short(sender),
				group = %Short(shared.group_id()),
				network = %Short(shared.network_id()),
				message = ?message,
				"received unexpected message as candidate",
			);
			return;
		};

		if !self.requested_from.contains(&sender) {
			tracing::warn!(
				peer = %Short(sender),
				group = %Short(shared.group_id()),
				network = %Short(shared.network_id()),
				term = %response.term,
				"ignoring vote response from peer we did not request vote from",
			);
			return;
		}

		match response.vote {
			Vote::Granted => {
				// If the peer granted its vote, we add it to the set of votes granted
				// to us. We will check if we have reached a quorum after processing the
				// vote and wake up any waiting
				self.votes_granted.insert(sender);
			}
			Vote::Abstained => {
				// If the peer abstained from voting, we remove it from the set of peers
				// we requested votes from, which effectively reduces the quorum
				// denominator and allows us to still win the election with a smaller
				// number of votes as long as we have a majority of the remaining voting
				// peers.
				self.requested_from.remove(&sender);
			}
			Vote::Denied => {}
		}

		if self.quorum_reached() {
			// If we have reached a quorum, we wake up all tasks that are waiting for
			// the election result so that they can transition to the leader state.
			for waker in self.wakers.drain(..) {
				waker.wake();
			}
		}
	}

	/// Checks if we have received votes from a quorum of peers to win the
	/// leader election.
	fn quorum_reached(&self) -> bool {
		let total_nodes = self.requested_from.len();
		let votes = self.votes_granted.len();
		let quorum = (total_nodes / 2) + 1;
		votes >= quorum
	}
}
