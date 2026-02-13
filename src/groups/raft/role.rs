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
				protocol::{RequestVoteResponse, Sync, Vote},
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
///
/// Messages that are common to all roles (e.g., stepping down on higher term,
/// voting for candidates, etc.) are handled at the `Role` level, and messages
/// that are specific to each role are forwarded to the role-specific message
#[derive(Debug, From)]
pub enum Role<M: StateMachine> {
	/// Passive state: responds to messages from candidates and leaders.
	/// If election timeout elapses without receiving `AppendEntries` from
	/// current leader or granting vote to candidate, converts to candidate.
	///
	/// Followers may serve read-only requests depending on the configured
	/// read consistency level. All log-mutating requests must be forwarded to
	/// the current leader.
	Follower(Follower<M>),

	/// Active state during elections: increments term, votes for self,
	/// sends `RequestVote` RPCs to all other servers, and waits for votes.
	///
	/// Nodes go into this state if they have not received `AppendEntries`
	/// messages from a leader within the election timeout.
	Candidate(Candidate<M>),

	/// Active state as leader: handles log-mutating requests from clients,
	/// replicates log entries, and sends periodic heartbeats to followers.
	Leader(Leader<M>),
}

impl<M: StateMachine> Role<M> {
	pub fn new<S: Storage<M::Command>>(shared: &Shared<S, M>) -> Self {
		Self::Follower(Follower::new(0, None, shared))
	}

	/// Drives the role-specific periodic actions (e.g., elections, heartbeats).
	pub fn poll_next_tick<S: Storage<M::Command>>(
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
	pub fn receive_protocol_message<S: Storage<M::Command>>(
		&mut self,
		message: Message<M::Command>,
		sender: PeerId,
		shared: &mut Shared<S, M>,
	) {
		if let Some(message_term) = message.term()
			&& message_term < self.term()
		{
			tracing::trace!(
				local_term = self.term(),
				message_term = message_term,
				group = %Short(shared.group_id()),
				network = %Short(shared.network_id()),
				sender = %Short(sender),
				"ignoring stale raft message"
			);
			return;
		}

		// Any message with a higher term should trigger an immediate step down to
		// follower state with the new term.
		self.maybe_step_down(&message, shared);

		// Handle `RequestVote` messages and cast votes if applicable. This is
		// common to all roles, as followers, candidates, and leaders can all
		// receive `RequestVote` messages. There is no more role-specific handling
		// for this message type.
		if self.maybe_cast_vote(&message, sender, shared) {
			// if the message was a `RequestVote` and we handled it by casting a
			// vote, then we don't need to forward it to the role-specific message
			// handlers.
			return;
		}

		// Handle some catch-up at the role level
		if Self::maybe_catchup_request(&message, sender, shared) {
			// if the message was a `Sync::*Request` message and we handled it by
			// processing the catch-up request, then we don't need to forward it to
			// the role-specific message handlers.
			return;
		}

		match self {
			Self::Follower(follower) => {
				follower.receive_protocol_message(message, sender, shared);
			}
			Self::Candidate(candidate) => {
				candidate.receive_protocol_message(message, sender, shared);
			}
			Self::Leader(leader) => {
				leader.receive_protocol_message(message, sender, shared);
			}
		}
	}

	/// Checks all incoming messages for a higher term and steps down to follower
	/// if necessary. Returns `true` if the node stepped down to follower state,
	/// otherwise `false`.
	fn maybe_step_down<S: Storage<M::Command>>(
		&mut self,
		message: &Message<M::Command>,
		shared: &Shared<S, M>,
	) {
		let Some(message_term) = message.term() else {
			// If the message does not carry a term, it cannot trigger a step down.
			return;
		};

		assert!(message_term >= self.term());

		if message_term > self.term() {
			if let Some(leader) = message.leader() {
				tracing::debug!(
					leader = %Short(leader),
					old_term = %self.term(),
					new_term = %message_term,
					group = %Short(shared.group_id()),
					network = %Short(shared.network_id()),
					"following",
				);
			} else {
				tracing::debug!(
					group = %Short(shared.group_id()),
					network = %Short(shared.network_id()),
					old_term = %self.term(),
					new_term = %message_term,
					"stepping down to follower",
				);
			}

			// If the incoming message has a higher term, we must step down to
			// follower state and follow the new leader (if provided), and process
			// the incoming message as a follower.
			*self = Follower::<M>::new::<S>(
				message_term, //
				message.leader(),
				shared,
			)
			.into();

			// notify status listeners that we have a new leader.
			shared.update_leader(message.leader());
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
	fn maybe_cast_vote<S: Storage<M::Command>>(
		&self,
		message: &Message<M::Command>,
		sender: PeerId,
		shared: &mut Shared<S, M>,
	) -> bool {
		let Message::RequestVote(request) = message else {
			return false;
		};

		// this should always hold because messages with lower terms are filtered
		// out in the `receive` method before reaching this point.
		assert!(request.term >= self.term());

		let local_cursor = shared.log.last();

		tracing::debug!(
			candidate = %Short(request.candidate),
			term = request.term,
			candidate_log = %request.log_position,
			local_log = %local_cursor,
			group = %Short(shared.group_id()),
			network = %Short(shared.network_id()),
			"new leader elections started by",
		);

		let bonds = shared.group.bonds.clone();
		let vote_with = |vote: Vote| {
			bonds.send_raft_message_to::<M>(
				Message::RequestVoteResponse(RequestVoteResponse {
					vote,
					term: request.term,
				}),
				sender,
			);
		};

		if !shared.should_vote(request.term, request.candidate) {
			// We have already voted for another candidate in the same term
			vote_with(Vote::Denied);
			return true;
		}

		if request.log_position.is_behind(&local_cursor) {
			// The candidate's log is not as up-to-date as ours, deny.
			vote_with(Vote::Denied);
			return true;
		}

		// If we reach this point, we can vote for the candidate. We record our vote
		// to prevent us from voting for multiple candidates in the same term and we
		// send a positive `RequestVoteResponse` back to the candidate.
		shared.cast_vote(request.term, sender);

		// check if this node is behind the candidate's log
		if local_cursor.is_behind(&request.log_position) {
			// We are behind the candidate — abstain rather than grant or deny,
			// so we don't inflate the voting committee with lagging nodes but also
			// don't object to the candidate winning the election and becoming leader.
			vote_with(Vote::Abstained);
		} else {
			// if we are fully caught up with the candidate's log, and we are ready to
			// become voting followers, then we grant a full vote to the candidate and
			// the candidate upon winning the elections will consider us to be part of
			// the initial quorum.
			vote_with(Vote::Granted);
		}

		true
	}

	/// Handles incoming `Sync` request messages that are part of the follower
	/// catch-up process.
	///
	/// Returns `true` if the message was handled at this level and should not be
	/// forwarded to the role-specific message handlers. This method only handles
	/// `DiscoveryRequest` and `FetchEntriesRequest` messages, the `*Response`
	/// variants are forwarded to the follower.
	fn maybe_catchup_request<S: Storage<M::Command>>(
		message: &Message<M::Command>,
		sender: PeerId,
		shared: &Shared<S, M>,
	) -> bool {
		let Message::Sync(message) = message else {
			return false;
		};

		match message {
			Sync::DiscoveryRequest => {
				// we only offer committed log entries to followers that are catching up
				let available = shared.log.available();
				let committed = shared.log.committed();
				let available = *available.start()..=committed.min(*available.end());

				tracing::trace!(
					peer = %Short(sender),
					range = ?available,
					group = %shared.group_id(),
					network = %shared.network_id(),
					"logs availability confirmed to"
				);

				let response = Message::Sync(Sync::DiscoveryResponse { available });
				shared.bonds().send_raft_message_to::<M>(response, sender);

				true
			}
			Sync::FetchEntriesRequest { range: _ } => true,
			Sync::DiscoveryResponse { .. } | Sync::FetchEntriesResponse { .. } => {
				// these messages are handled by the follower role, so we return false
				// to forward them to the role-specific message handlers.
				false
			}
		}
	}
}

impl<M: StateMachine> Role<M> {
	pub const fn term(&self) -> Term {
		match self {
			Self::Follower(follower) => follower.term(),
			Self::Candidate(candidate) => candidate.term(),
			Self::Leader(leader) => leader.term(),
		}
	}
}

impl<M: StateMachine> fmt::Display for Role<M> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Follower(_) => write!(f, "Follower"),
			Self::Candidate(_) => write!(f, "Candidate"),
			Self::Leader(_) => write!(f, "Leader"),
		}
	}
}
