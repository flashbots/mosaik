use {
	crate::{
		PeerId,
		groups::{
			log::{StateMachine, Storage, Term},
			raft::{
				Message,
				candidate::Candidate,
				follower::Follower,
				leader::Leader,
				protocol::RequestVoteResponse,
				shared::Shared,
			},
		},
		primitives::Short,
	},
	core::{
		fmt,
		ops::ControlFlow,
		task::{Context, Poll},
	},
	derive_more::From,
};

/// Raft node role - each node is always in one of these states.
///
/// Depending on the currently assumed role, protocol messages are handled
/// differently and certain actions are taken (e.g., starting elections,
/// sending heartbeats, etc.).
#[derive(Debug, From)]
pub enum Role<S: Storage<M::Command>, M: StateMachine> {
	/// Passive state: responds to messages from candidates and leaders.
	/// If election timeout elapses without receiving `AppendEntries` from
	/// current leader or granting vote to candidate, converts to candidate.
	///
	/// Followers may serve read-only requests depending on the configured
	/// read consistency level. All log-mutating requests must be forwarded to
	/// the current leader.
	Follower(Follower<S, M>),

	/// Active state during elections: increments term, votes for self,
	/// sends `RequestVote` RPCs to all other servers, and waits for votes.
	///
	/// Nodes go into this state if they have not received `AppendEntries`
	/// messages from a leader within the election timeout.
	Candidate(Candidate<S, M>),

	/// Active state as leader: handles log-mutating requests from clients,
	/// replicates log entries, and sends periodic heartbeats to followers.
	Leader(Leader<S, M>),
}

impl<S: Storage<M::Command>, M: StateMachine> Role<S, M> {
	pub fn new(shared: &Shared<S, M>) -> Self {
		Self::Follower(Follower::new(0, None, shared))
	}

	/// Drives the role-specific periodic actions (e.g., elections, heartbeats).
	pub fn poll_next_tick(
		&mut self,
		cx: &mut Context<'_>,
		shared: &mut Shared<S, M>,
	) -> Poll<Option<()>> {
		let next_step = match self {
			Self::Follower(follower) => follower.poll_next_tick(cx, shared),
			Self::Candidate(candidate) => candidate.poll_next_tick(cx, shared),
			Self::Leader(leader) => leader.poll_next_tick(cx, shared),
		};

		match next_step {
			Poll::Ready(next) => {
				if let ControlFlow::Break(next_role) = next {
					// transition to the next role if the current role's tick indicates a
					// role change (e.g., election timeout elapsed, new leader elected,
					// etc.)
					*self = next_role;
				}
				Poll::Ready(Some(()))
			}
			Poll::Pending => Poll::Pending,
		}
	}

	/// Handles incoming consensus protocol messages based on the current role.
	/// Implements behaviors common to all roles, such as stepping down on
	/// receiving messages with higher terms.
	pub fn receive(
		&mut self,
		message: Message<M::Command>,
		sender: PeerId,
		shared: &mut Shared<S, M>,
	) {
		if message.term() < self.term() {
			tracing::trace!(
				local_term = self.term(),
				message_term = message.term(),
				group = %Short(shared.group().group_id()),
				network = %Short(shared.group().network_id()),
				sender = %Short(sender),
				"ignoring stale message"
			);
			return;
		}

		// each message may cause us to step down to follower state
		self.maybe_step_down(&message, shared);

		// check if someone with a higher term has started an election and cast our
		// vote if we haven't already
		if !self.maybe_cast_vote(&message, sender, shared) {
			match self {
				Self::Follower(follower) => follower.receive(message, sender, shared),
				Self::Candidate(candidate) => {
					candidate.receive(message, sender, shared);
				}
				Self::Leader(leader) => leader.receive(message, sender, shared),
			}
		}
	}

	/// Checks all incoming messages for a higher term and steps down to follower
	/// if necessary. Returns `true` if the node stepped down to follower state,
	/// otherwise `false`.
	fn maybe_step_down(
		&mut self,
		message: &Message<M::Command>,
		shared: &Shared<S, M>,
	) {
		assert!(message.term() >= self.term());

		if message.term() > self.term() {
			if let Some(leader) = message.leader() {
				tracing::debug!(
					leader = %Short(leader),
					group = %Short(shared.group().group_id()),
					network = %Short(shared.group().network_id()),
					old_term = %self.term(),
					new_term = %message.term(),
					"following",
				);
			} else {
				tracing::debug!(
					group = %Short(shared.group().group_id()),
					network = %Short(shared.group().network_id()),
					old_term = %self.term(),
					new_term = %message.term(),
					"stepping down to follower",
				);
			}

			// If the incoming message has a higher term, we must step down to
			// follower state and follow the new leader (if provided).
			*self = Follower::new(message.term(), message.leader(), shared).into();

			// notify status listeners that we have a new leader.
			shared.group().when.update_leader(message.leader());
		}
	}

	/// Handles incoming `RequestVote` messages by deciding whether to cast a vote
	/// for the candidate based on the Raft voting rules. This behavior is common
	/// to all roles, as followers, candidates, and leaders can all receive
	/// `RequestVote` messages and may need to cast votes for candidates with
	/// higher terms.
	///
	/// returns true if the message was handled (i.e., it was a `RequestVote`
	/// message and should not be forwarded to other roles), otherwise false and
	/// the message will be forwarded to the role-specific message handler.
	fn maybe_cast_vote(
		&self,
		message: &Message<M::Command>,
		sender: PeerId,
		shared: &mut Shared<S, M>,
	) -> bool {
		assert!(message.term() >= self.term());
		let Message::RequestVote(request) = message else {
			return false;
		};

		let deny_vote = || {
			shared.group().bonds.send_raft_message_to::<M>(
				Message::RequestVoteResponse(RequestVoteResponse {
					term: request.term,
					vote_granted: false,
				}),
				sender,
			);
		};

		if !shared.should_vote(request.term, request.candidate) {
			// We have already voted for another candidate in the same term
			deny_vote();
			return true;
		}

		let (term, index) = shared.log().last();
		if request.last_log_term < term || request.last_log_index < index {
			// The candidate's log is not as up-to-date as ours, so we should not
			// vote for it.
			deny_vote();
			return true;
		}

		// If we reach this point, we can vote for the candidate. We record our vote
		// to prevent us from voting for multiple candidates in the same term and we
		// send a positive `RequestVoteResponse` back to the candidate.
		shared.cast_vote(request.term, sender);
		shared.group().bonds.send_raft_message_to::<M>(
			Message::RequestVoteResponse(RequestVoteResponse {
				term: request.term,
				vote_granted: true,
			}),
			sender,
		);

		true
	}
}

impl<S: Storage<M::Command>, M: StateMachine> Role<S, M> {
	pub const fn term(&self) -> Term {
		match self {
			Self::Follower(follower) => follower.term(),
			Self::Candidate(candidate) => candidate.term(),
			Self::Leader(leader) => leader.term(),
		}
	}
}

impl<S: Storage<M::Command>, M: StateMachine> fmt::Display for Role<S, M> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Follower(_) => write!(f, "Follower"),
			Self::Candidate(_) => write!(f, "Candidate"),
			Self::Leader(_) => write!(f, "Leader"),
		}
	}
}
