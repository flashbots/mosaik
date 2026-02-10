use {
	crate::{
		PeerId,
		groups::{
			log::{StateMachine, Storage, Term},
			raft::{Message, candidate::Candidate, role::Role, shared::Shared},
		},
		primitives::Short,
	},
	core::{
		marker::PhantomData,
		ops::ControlFlow,
		pin::Pin,
		task::{Context, Poll},
	},
	std::time::Instant,
	tokio::time::Sleep,
};

/// In the follower role, the node is passive and responds to messages from
/// candidates and leaders. If the election timeout elapses without receiving
/// `AppendEntries` from the current leader, the follower transitions to
/// leader candidate state and starts a new election.
#[derive(Debug)]
pub struct Follower<S: Storage<M::Command>, M: StateMachine> {
	/// The current term for this node.
	term: Term,

	/// The current leader that this follower is following (if any). This is
	/// updated whenever we receive a valid `AppendEntries` message from a
	/// leader.
	leader: Option<PeerId>,

	/// The election timeout for this follower. If we do not receive any
	/// messages from a valid leader within this timeout, we will transition to
	/// candidate state and start a new election.
	election_timeout: Pin<Box<Sleep>>,

	#[doc(hidden)]
	_marker: PhantomData<(S, M)>,
}

impl<S: Storage<M::Command>, M: StateMachine> Follower<S, M> {
	/// Creates a new follower role for the specified term
	pub fn new(
		term: Term,
		leader: Option<PeerId>,
		shared: &Shared<S, M>,
	) -> Self {
		let mut election_timeout = shared.config().intervals().election_timeout();

		if term == 0 {
			// for the initial term, we introduce an additional bootstrap delay to
			// give all nodes enough time to start up and discover each other before
			// triggering the first election.
			election_timeout += shared.config().intervals().bootstrap_delay;
		}

		Self {
			term,
			leader,
			election_timeout: Box::pin(tokio::time::sleep(election_timeout)),
			_marker: PhantomData,
		}
	}

	/// Returns the current term of this follower.
	pub const fn term(&self) -> Term {
		self.term
	}
}

impl<S: Storage<M::Command>, M: StateMachine> Follower<S, M> {
	/// As a follower, we wait for messages from leaders or candidates.
	/// If no messages are received within the election timeout, we transition to
	/// a candidate state and start a new election.
	///
	/// Returns `Poll::Ready(ControlFlow::Break(new_role))` if the role should
	/// transition to a new state (e.g., candidate) or `Poll::Pending` if it
	/// should continue waiting in the follower state.
	pub fn poll_next_tick(
		&mut self,
		cx: &mut Context<'_>,
		shared: &mut Shared<S, M>,
	) -> Poll<ControlFlow<Role<S, M>>> {
		if self.election_timeout.as_mut().poll(cx).is_ready() {
			// election timeout elapsed without hearing from a leader, so we start
			// a new election by transitioning to candidate state
			return Poll::Ready(ControlFlow::Break(
				Candidate::new(self.term + 1, shared).into(),
			));
		}

		Poll::Pending
	}

	/// In follower role there is only one message type that we expect to
	/// receive and handle at the role-specific level: `AppendEntries` from a
	/// leader. All other message types (e.g., `RequestVote` from candidates) are
	/// handled at the shared level since they can be received in any role and do
	/// not require any role-specific state to be processed.
	///
	/// Upon receiving a valid `AppendEntries` message from a leader, we reset the
	/// election timeout and update the current leader for this follower.
	pub fn receive(
		&mut self,
		message: Message<M::Command>,
		sender: PeerId,
		shared: &Shared<S, M>,
	) {
		let Message::AppendEntries(request) = message else {
			tracing::warn!(
				term = self.term(),
				group = %Short(shared.group().group_id()),
				network = %Short(shared.group().network_id()),
				"unexpected message from {}: only AppendEntries is expected in follower state",
				Short(sender),
			);
			return;
		};

		// signal to public api status listeners a potential update
		// to the current leader for this follower.
		self.leader = Some(request.leader);
		shared.group().when.update_leader(Some(request.leader));

		// each valid `AppendEntries` message from a leader resets the election
		// timeout and updates the current leader for this follower
		let next_election_timeout =
			Instant::now() + shared.group().config.intervals().election_timeout();

		self
			.election_timeout
			.as_mut()
			.reset(next_election_timeout.into());
	}
}
