use {
	crate::{
		NetworkId,
		PeerId,
		groups::{
			Bonds,
			GroupId,
			Index,
			IntervalsConfig,
			When,
			config::GroupConfig,
			log::{self, Term},
			state::WorkerState,
		},
		primitives::Short,
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
	pub group: Arc<WorkerState>,

	/// The persistent log for this group that tracks all changes to the group's
	/// replicated state machine through raft.
	pub log: log::Driver<S, M>,

	/// The last vote casted by the local node in leader elections.
	pub last_vote: Option<(Term, PeerId)>,
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

	/// Returns the group configuration for this consensus group.
	pub fn config(&self) -> &GroupConfig {
		&self.group.config
	}

	/// Returns the timing intervals configuration for this consensus group.
	pub fn intervals(&self) -> &IntervalsConfig {
		self.config().intervals()
	}

	/// Returns the list of active bonds for this consensus group.
	pub fn bonds(&self) -> &Bonds {
		&self.group.bonds
	}

	/// Returns the `When` event emitter for this consensus group, which can be
	/// used to await changes to the group's state, such as leadership changes or
	/// log commitment.
	pub fn when(&self) -> &When {
		&self.group.when
	}

	/// Returns the local ID of this node in the consensus group.
	pub fn local_id(&self) -> PeerId {
		self.group.local_id()
	}

	/// Returns the group ID of this consensus group.
	pub fn group_id(&self) -> &GroupId {
		self.group.group_id()
	}

	/// Returns the network ID of this consensus group.
	pub fn network_id(&self) -> &NetworkId {
		self.group.network_id()
	}

	/// Updates the leader information in the group state.
	pub fn update_leader(&self, leader: Option<PeerId>) {
		self.group.when.update_leader(leader);
	}

	/// Updates the online status of the local node in the group state to
	/// online. See [`When::is_online`] for more details on what it means for a
	/// node to be online.
	pub fn set_online(&self) {
		self.group.when.set_online_status(true);
	}

	/// Updates the online status of the local node in the group state to
	/// offline. See [`When::is_offline`] for more details on what it means for a
	/// node to be offline.
	pub fn set_offline(&self) {
		self.group.when.set_online_status(false);
	}

	/// Updates the committed index in the group public api observers. This should
	/// be called whenever the committed index of the log advances, which is when
	/// new entries are applied to the state machine and become visible to the
	/// external world through queries. This allows external observers to track
	/// the progress of the log and know when their commands have been committed
	/// and applied to the state machine.
	pub fn update_committed(&self, committed: Index) {
		self.group.when.update_committed(committed);
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
	pub fn cast_vote(&mut self, term: Term, candidate: PeerId) {
		self.last_vote = Some((term, candidate));

		tracing::debug!(
			candidate = %Short(candidate),
			term,
			group = %Short(self.group_id()),
			network = %Short(self.network_id()),
			"casted vote for leader",
		);
	}
}
