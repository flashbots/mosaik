use {
	crate::{
		PeerId,
		groups::{
			consensus::{
				ConsensusMessage,
				candidate::Candidate,
				follower::Follower,
				leader::Leader,
				shared::Shared,
			},
			log::{StateMachine, Storage, Term},
		},
		primitives::Short,
	},
	derive_more::From,
};

/// Raft node role - each node is always in one of these states.
///
/// Depending on the currently assumed role, protocol messages are handled
/// differently and certain actions are taken (e.g., starting elections,
/// sending heartbeats, etc.).
#[derive(Debug, From)]
pub enum Role {
	/// Passive state: responds to messages from candidates and leaders.
	/// If election timeout elapses without receiving `AppendEntries` from
	/// current leader or granting vote to candidate, converts to candidate.
	///
	/// Followers may serve read-only requests depending on the configured
	/// read consistency level. All log-mutating requests must be forwarded to
	/// the current leader.
	Follower(Follower),

	/// Active state during elections: increments term, votes for self,
	/// sends `RequestVote` RPCs to all other servers, and waits for votes.
	///
	/// Nodes go into this state if they have not received `AppendEntries`
	/// messages from a leader within the election timeout.
	Candidate(Candidate),

	/// Active state as leader: handles log-mutating requests from clients,
	/// replicates log entries, and sends periodic heartbeats to followers.
	Leader(Leader),
}

impl Default for Role {
	fn default() -> Self {
		Self::Follower(Follower::new(0, None))
	}
}

impl Role {
	/// Drives the role-specific periodic actions (e.g., elections, heartbeats).
	pub async fn tick<S: Storage<M::Command>, M: StateMachine>(
		&mut self,
		shared: &mut Shared<S, M>,
	) {
		match self {
			Role::Follower(follower) => follower.tick(shared).await,
			Role::Candidate(candidate) => candidate.tick(shared).await,
			Role::Leader(leader) => leader.tick(shared).await,
		}
	}

	/// Handles incoming consensus protocol messages based on the current role.
	/// Implements behaviors common to all roles, such as stepping down on
	/// receiving messages with higher terms.
	pub fn receive<S: Storage<M::Command>, M: StateMachine>(
		&mut self,
		message: ConsensusMessage,
		sender: PeerId,
		shared: &mut Shared<S, M>,
	) {
		// each message may cause us to step down to follower state
		self.maybe_step_down(&message, shared);

		match self {
			Role::Follower(follower) => follower.receive(message, sender, shared),
			Role::Candidate(candidate) => candidate.receive(message, sender, shared),
			Role::Leader(leader) => leader.receive(message, sender, shared),
		}
	}

	/// Checks all incoming messages for a higher term and steps down to follower
	/// if necessary. Returns `true` if the node stepped down to follower state,
	/// otherwise `false`.
	fn maybe_step_down<S: Storage<M::Command>, M: StateMachine>(
		&mut self,
		message: &ConsensusMessage,
		shared: &mut Shared<S, M>,
	) {
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
			*self = Follower::new(message.term(), message.leader()).into();
		}
	}
}

impl Role {
	pub fn term(&self) -> Term {
		match self {
			Role::Follower(follower) => follower.term(),
			Role::Candidate(candidate) => candidate.term(),
			Role::Leader(leader) => leader.term(),
		}
	}
}
