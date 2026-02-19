use {
	crate::{
		NetworkId,
		PeerId,
		groups::{
			Bonds,
			ConsensusConfig,
			Cursor,
			GroupId,
			Index,
			StateMachine,
			StateSync,
			StateSyncContext,
			StateSyncProvider,
			config::GroupConfig,
			log::{self, Term},
			raft::Message,
			state::WorkerState,
		},
	},
	core::mem::MaybeUninit,
	std::sync::Arc,
};

/// State that is shared across all raft roles.
pub struct Shared<S, M>
where
	S: log::Storage<M::Command>,
	M: StateMachine,
{
	/// The group state that is shared between the long-running group worker and
	/// the external world.
	pub group: Arc<WorkerState>,

	/// The persistent log for this group that tracks all changes to the group's
	/// replicated state machine through raft.
	pub log: log::Driver<S, M>,

	/// The last vote casted by the local node in leader elections.
	pub last_vote: Option<(Term, PeerId)>,

	/// Wakers for tasks that are waiting for changes in the shared state, such
	/// as leadership changes or log commitment.
	pub wakers: Vec<std::task::Waker>,

	/// State machine-specific state sync provider.
	///
	/// This is used to create new state sync sessions for followers and one
	/// state sync provider per group instance for all roles.
	sync: M::StateSync,

	/// State machine-specific state sync provider.
	///
	/// Applicable to all roles.
	sync_provider: MaybeUninit<<M::StateSync as StateSync>::Provider>,
}

impl<S, M> Shared<S, M>
where
	S: log::Storage<M::Command>,
	M: StateMachine,
{
	pub(super) fn new(
		group: Arc<WorkerState>,
		storage: S,
		state_machine: M,
	) -> Self {
		let mut instance = Self {
			group,
			last_vote: None,
			sync: state_machine.state_sync(),
			log: log::Driver::new(storage, state_machine),
			wakers: Vec::new(),
			sync_provider: MaybeUninit::uninit(),
		};

		let provider = instance.sync.create_provider(&instance);
		instance.sync_provider.write(provider);

		instance
	}

	/// Returns the group configuration for this consensus group.
	pub fn config(&self) -> &GroupConfig {
		&self.group.config
	}

	/// Returns the timing intervals configuration for this consensus group.
	pub fn consensus(&self) -> &ConsensusConfig {
		self.config().consensus()
	}

	/// Returns the list of active bonds for this consensus group.
	pub fn bonds(&self) -> &Bonds {
		&self.group.bonds
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

	/// Optionally handles an incoming state sync message at the handler level
	pub fn sync_provider_receive(
		&mut self,
		message: <M::StateSync as StateSync>::Message,
		sender: PeerId,
	) -> Result<(), <M::StateSync as StateSync>::Message> {
		// SAFETY: The `sync_provider` field is initialized in the constructor of
		// `Shared` and is never mutated after that, so it is safe to assume that
		// it is always initialized when accessed through this method.
		let provider: *mut _ = unsafe { self.sync_provider.assume_init_mut() };

		// SAFETY: There is no public API on `StateSyncContext` that allows the
		// mutation of the `sync_provider` field of `Shared`.
		let provider = unsafe { &mut *provider };

		// Forward the message to the state sync provider. If the provider consumes
		// the message, then it will not be forwarded to the state sync session of
		// the follower.
		provider.receive(message, sender, self)
	}

	/// Creates a new state synchronization session for a lagging follower that is
	/// about to enter the state catch-up process.
	pub fn create_sync_session(
		&mut self,
		position: Cursor,
		leader_commit: Index,
		entries: Vec<(M::Command, Term)>,
	) -> <M::StateSync as StateSync>::Session {
		let ptr: *mut M::StateSync = &raw mut self.sync;

		// SAFETY: There is no public API on `StateSyncContext` that allows the
		// mutation of the `sync` field of `Shared`.
		let sync = unsafe { &mut *ptr };
		sync.create_session(self, position, leader_commit, entries)
	}

	/// Queries the state sync provider for the log index up to which it is safe
	/// to prune log entries, and prunes them if the provider indicates it is
	/// safe to do so.
	///
	/// This should be called after the committed index advances, as pruning is
	/// only meaningful for committed entries that the provider has determined
	/// are no longer needed for state synchronization.
	pub fn prune_safe_prefix(&mut self) {
		// SAFETY: The `sync_provider` field is initialized in the constructor of
		// `Shared` and is never mutated after that, so it is safe to assume that
		// it is always initialized when accessed through this method.
		let provider: *mut _ = unsafe { self.sync_provider.assume_init_mut() };

		// SAFETY: There is no public API on `StateSyncContext` that allows the
		// mutation of the `sync_provider` field of `Shared`.
		let provider = unsafe { &mut *provider };

		if let Some(up_to) = provider.safe_to_prune_prefix(self) {
			self.log.prune_prefix(up_to);
		}
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
	/// be called whenever the committed index of the log advances.
	pub fn update_committed(&mut self, committed: Index) {
		self.group.when.update_committed(committed);

		// SAFETY: The `sync_provider` field is initialized in the constructor of
		// `Shared` and is never mutated after that, so it is safe to assume that
		// it is always initialized when accessed through this method.
		let provider: *mut _ = unsafe { self.sync_provider.assume_init_mut() };

		// SAFETY: There is no public API on `StateSyncContext` that allows the
		// mutation of the `sync_provider` field of `Shared`.
		let provider = unsafe { &mut *provider };

		// Notify the state sync provider that the committed index has advanced.
		provider.committed(committed, self);
	}

	/// Updates the log position in the group public api observers. This should be
	/// called whenever the local node's log position changes, which can happen
	/// when new log entries are appended to the log or when the log is truncated.
	pub fn update_log_pos(&self, log_pos: Cursor) {
		self.group.when.update_log_pos(log_pos);
	}

	/// Called when we receive a `RequestVote` message from a candidate. This
	/// checks if we have already voted for another candidate in the same term.
	pub fn can_vote(&self, term: Term, candidate: PeerId) -> bool {
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
	pub const fn save_vote(&mut self, term: Term, candidate: PeerId) {
		self.last_vote = Some((term, candidate));
	}

	pub fn add_waker(&mut self, waker: std::task::Waker) {
		self.wakers.push(waker);
	}

	pub fn wake_all(&mut self) {
		for waker in self.wakers.drain(..) {
			waker.wake();
		}
	}
}

impl<S, M> crate::primitives::sealed::Sealed for Shared<S, M>
where
	S: log::Storage<M::Command>,
	M: StateMachine,
{
}

impl<S, M> StateSyncContext<M::StateSync> for Shared<S, M>
where
	S: log::Storage<M::Command>,
	M: StateMachine,
{
	fn machine(&self) -> &<M::StateSync as StateSync>::Machine {
		self.log.machine()
	}

	fn machine_mut(&mut self) -> &mut <M::StateSync as StateSync>::Machine {
		self.log.machine_mut()
	}

	fn log(&self) -> &dyn log::Storage<M::Command> {
		self.log.storage()
	}

	fn log_mut(&mut self) -> &mut dyn log::Storage<M::Command> {
		self.log.storage_mut()
	}

	fn committed(&self) -> Index {
		self.log.committed()
	}

	fn local_id(&self) -> PeerId {
		self.group.local_id()
	}

	fn group_id(&self) -> GroupId {
		*self.group.group_id()
	}

	fn network_id(&self) -> NetworkId {
		*self.group.network_id()
	}

	fn leader(&self) -> Option<PeerId> {
		self.group.when.current_leader()
	}

	fn send_to(
		&mut self,
		peer: PeerId,
		message: <M::StateSync as StateSync>::Message,
	) {
		self
			.group
			.bonds
			.send_raft_to::<M>(&Message::StateSync(message), peer);
	}

	fn broadcast(&mut self, message: <M::StateSync as StateSync>::Message) {
		self
			.group
			.bonds
			.broadcast_raft::<M>(&Message::<M>::StateSync(message));
	}

	fn bonds(&self) -> Bonds {
		self.group.bonds.clone()
	}
}
