use {
	crate::{
		PeerId,
		groups::{
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

	pub const fn log(&self) -> &log::Driver<S, M> {
		&self.log
	}
}
