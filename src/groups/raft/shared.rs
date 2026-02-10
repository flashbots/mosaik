use {
	crate::{
		PeerId,
		groups::{
			config::GroupConfig,
			log::{self, Term},
			state::WorkerState,
		},
	},
	std::sync::Arc,
};

/// State that is shared across all raft roles.
pub struct Shared<S, M>
where
	S: log::Storage<M::Command>,
	M: log::StateMachine,
{
	/// The group state that is shared between the long-running group worker and
	/// the external world.
	group: Arc<WorkerState>,

	/// The persistent log for this group that tracks all changes to the group's
	/// replicated state machine through raft.
	log: log::Driver<S, M>,

	/// The last vote casted by the local node in leader elections.
	last_vote: Option<(Term, PeerId)>,
}

impl<S, M> Shared<S, M>
where
	S: log::Storage<M::Command>,
	M: log::StateMachine,
{
	pub(super) const fn new(
		group: Arc<WorkerState>,
		storage: S,
		state_machine: M,
	) -> Self {
		Self {
			group,
			log: log::Driver::new(storage, state_machine),
			last_vote: None,
		}
	}

	pub fn group(&self) -> &WorkerState {
		&self.group
	}

	pub fn config(&self) -> &GroupConfig {
		&self.group.config
	}

	pub const fn log(&self) -> &log::Driver<S, M> {
		&self.log
	}

	/// Called when we receive a `RequestVote` message from a candidate. This
	/// checks if we have already voted for another candidate in the same term.
	pub fn should_vote(&self, term: Term, candidate: PeerId) -> bool {
		let Some((last_term, last_candidate)) = self.last_vote else {
			// If we haven't casted any vote yet, we can vote for the candidate.
			return true;
		};

		if last_term < term {
			// If the incoming vote request is for a higher term than the last
			// vote we casted, we can vote for the new candidate.
			return true;
		}

		if last_term == term && last_candidate == candidate {
			// If the incoming vote request is for the same term and candidate as
			// the last vote we casted, we can vote for the candidate again. This does
			// not constitute equivocation.
			return true;
		}

		// In all other cases, we should not vote for the candidate.
		false
	}

	/// Records the fact that we casted a vote for the given candidate in this
	/// term. This is used to prevent us from voting for multiple candidates in
	/// the same term.
	pub const fn cast_vote(&mut self, term: Term, candidate: PeerId) {
		self.last_vote = Some((term, candidate));
	}
}
