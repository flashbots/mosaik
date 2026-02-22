use {
	crate::{
		NetworkId,
		PeerId,
		groups::{
			ApplyContext,
			Bonds,
			ConsensusConfig,
			Cursor,
			GroupId,
			Index,
			StateMachine,
			StateSync,
			StateSyncProvider,
			Storage,
			SyncContext,
			SyncProviderContext,
			SyncSessionContext,
			Term,
			config::GroupConfig,
			raft::Message,
			state::{WorkerRaftCommand, WorkerState},
		},
	},
	core::{mem::MaybeUninit, task::Poll},
	std::sync::Arc,
	tokio::sync::{
		mpsc::{UnboundedSender, error::SendError},
		oneshot,
	},
};

/// State that is shared across all raft roles.
pub struct Shared<S, M>
where
	S: Storage<M::Command>,
	M: StateMachine,
{
	/// The group state that is shared between the long-running group worker and
	/// the external world.
	pub group: Arc<WorkerState>,

	/// The underlying storage for the log entries of this group state machine.
	pub storage: S,

	/// The instance of the state machine of this group that applies committed
	/// log entries and responds to queries.
	pub state_machine: M,

	/// The last vote casted by the local node in leader elections.
	pub last_vote: Option<(Term, PeerId)>,

	/// Wakers for tasks that are waiting for changes in the shared state, such
	/// as leadership changes or log commitment.
	pub wakers: Vec<std::task::Waker>,

	/// State machine-specific state sync provider.
	///
	/// This is used to create new state sync sessions for followers and one
	/// state sync provider per group instance for all roles.
	pub sync: M::StateSync,

	/// State machine-specific state sync provider.
	///
	/// Applicable to all roles.
	pub sync_provider: MaybeUninit<<M::StateSync as StateSync>::Provider>,

	/// The index and term of the latest replicated and committed log entry that
	/// has been applied to the state machine.
	committed: Cursor,
}

impl<S, M> Shared<S, M>
where
	S: Storage<M::Command>,
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
			storage,
			sync: state_machine.state_sync(),
			wakers: Vec::new(),
			sync_provider: MaybeUninit::uninit(),
			state_machine,
			committed: Cursor::default(),
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

	/// Returns the index of the last committed log entry that has been replicated
	/// to a quorum and applied to the state machine.
	pub const fn committed(&self) -> Cursor {
		self.committed
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

		if !self.committed().index().is_zero()
			&& let Some(up_to) = provider.safe_to_prune_prefix(self)
		{
			self
				.storage
				.prune_prefix(up_to.min(self.committed().index().prev()));
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
	pub fn update_committed(&mut self, committed: Cursor) {
		self.group.when.update_committed(committed.index());

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

	/// Commits log entries up to the given index and applies them to the state
	/// machine. This should only be called with an index that has been replicated
	/// to a majority of followers, which is guaranteed by the Raft leader before
	/// calling this method.
	///
	/// Returns the index of the latest committed entry after this operation,
	/// which may be less than the given index if it goes beyond the end of the
	/// log.
	pub fn commit_up_to(&mut self, index: Index) -> Cursor {
		let committed = self.committed();
		let range = committed.index().next()..=index;
		let commands = self.storage.get_range(&range);
		let log_position = self.storage.last();

		// Partition commands into consecutive batches by term and apply each
		// batch with the correct `current_term` in the apply context.
		let mut iter = commands.into_iter().peekable();

		while iter.peek().is_some() {
			let batch_term = iter.peek().unwrap().0;
			let batch: Vec<_> = core::iter::from_fn(|| {
				if iter.peek().map(|(t, _, _)| *t) == Some(batch_term) {
					iter.next()
				} else {
					None
				}
			})
			.collect();

			let batch_size = batch.len() as u64;
			let ctx = LocalApplyContext {
				prev_committed: self.committed,
				log_position,
				term: batch_term,
			};

			self
				.state_machine
				.apply_batch(batch.into_iter().map(|(_, _, cmd)| cmd), &ctx);

			self.committed =
				Cursor::new(batch_term, self.committed.index() + batch_size);
		}

		self.committed
	}

	/// Polls the state sync provider for any pending work it needs to do on every
	/// tick of the raft worker loop.
	pub fn poll_state_sync_provider(
		&mut self,
		cx: &mut std::task::Context<'_>,
	) -> Poll<()> {
		// SAFETY: The `sync_provider` field is initialized in the constructor of
		// `Shared` and is never mutated after that, so it is safe to assume that
		// it is always initialized when accessed through this method.
		let provider: *mut _ = unsafe { self.sync_provider.assume_init_mut() };

		// SAFETY: There is no public API on `StateSyncContext` that allows the
		// mutation of the `sync_provider` field of `Shared`.
		let provider = unsafe { &mut *provider };
		provider.poll(cx, self)
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
	S: Storage<M::Command>,
	M: StateMachine,
{
}

impl<S, M> SyncContext<M::StateSync> for Shared<S, M>
where
	S: Storage<M::Command>,
	M: StateMachine,
{
	fn state_machine(&self) -> &<M::StateSync as StateSync>::Machine {
		&self.state_machine
	}

	fn state_machine_mut(&mut self) -> &mut <M::StateSync as StateSync>::Machine {
		&mut self.state_machine
	}

	fn log(&self) -> &dyn Storage<M::Command> {
		&self.storage
	}

	fn log_mut(&mut self) -> &mut dyn Storage<M::Command> {
		&mut self.storage
	}

	fn committed(&self) -> Cursor {
		self.committed
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

	fn broadcast(
		&mut self,
		message: <M::StateSync as StateSync>::Message,
	) -> Vec<PeerId> {
		self
			.group
			.bonds
			.broadcast_raft::<M>(&Message::<M>::StateSync(message))
	}

	fn bonds(&self) -> Bonds {
		self.group.bonds.clone()
	}
}

impl<S, M> SyncSessionContext<M::StateSync> for Shared<S, M>
where
	S: Storage<M::Command>,
	M: StateMachine,
{
	fn leader(&self) -> PeerId {
		self
			.group
			.when
			.current_leader()
			.expect("leader always known on lagging followers during sync sessions")
	}

	fn set_committed(&mut self, position: Cursor) {
		self.committed = position;
		self.group.when.update_committed(position.index());
	}
}

impl<S, M> SyncProviderContext<M::StateSync> for Shared<S, M>
where
	S: Storage<M::Command>,
	M: StateMachine,
{
	fn leader(&self) -> Option<PeerId> {
		self.group.when.current_leader()
	}

	fn feed_command(&mut self, command: M::Command) -> Result<(), M::Command> {
		if self.is_leader() {
			let sender = self
				.group
				.raft_cmd_tx
				.downcast_ref::<UnboundedSender<WorkerRaftCommand<M>>>()
				.expect("invalid raft_tx type. this is a bug");

			let (unused, _) = oneshot::channel();
			if let Err(SendError(WorkerRaftCommand::Feed(commands, _))) =
				sender.send(WorkerRaftCommand::Feed(vec![command], unused))
			{
				return Err(
					commands
						.into_iter()
						.next()
						.expect("just sent this command, so it must be here"),
				);
			}
			Ok(())
		} else {
			Err(command)
		}
	}
}

#[derive(Debug)]
struct LocalApplyContext {
	prev_committed: Cursor,
	log_position: Cursor,
	term: Term,
}

impl ApplyContext for LocalApplyContext {
	fn committed(&self) -> Cursor {
		self.prev_committed
	}

	fn log_position(&self) -> Cursor {
		self.log_position
	}

	fn current_term(&self) -> Term {
		self.term
	}
}
