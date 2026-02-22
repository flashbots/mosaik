use {
	crate::{
		PeerId,
		groups::{
			Bonds,
			Cursor,
			GroupId,
			Index,
			IndexRange,
			StateMachine,
			When,
			config::GroupConfig,
			error::{CommandError, QueryError},
			state::WorkerRaftCommand,
		},
		primitives::Short,
	},
	core::{fmt, marker::PhantomData},
	dashmap::DashMap,
	derive_more::Deref,
	serde::{Deserialize, Serialize},
	state::WorkerState,
	std::sync::Arc,
	tokio::sync::{
		mpsc::{UnboundedSender, error::SendError},
		oneshot,
	},
};

pub(in crate::groups) mod state;
pub(super) mod worker;

/// Query consistency levels for group state machine queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consistency {
	/// The query will be processed by the local node up to the latest known
	/// committed state without any guarantee of consistency with the current
	/// state of the group. This is the fastest way to process a query, but it
	/// may return stale or inconsistent data.
	Weak,

	/// The query will be forwarded to the current leader of the group for
	/// processing, which guarantees that the result is consistent with the
	/// current state of the group. However, this may introduce additional
	/// latency if the local node is not the leader or if there are network
	/// issues.
	Strong,
}

/// Carries the result of executing a query against the group's state machine,
/// along with the position of the state machine at which the query was
/// executed.
#[derive(Debug, Clone, Deref, Serialize, Deserialize)]
pub struct CommittedQueryResult<M: StateMachine> {
	/// The result of executing the query against the state machine.
	#[deref]
	pub result: M::QueryResult,

	/// The index of the latest committed command in the log at the time the
	/// query was processed. This can be used by clients to determine how up to
	/// date the query result is with respect to the current state of the group.
	pub at_position: Index,
}

/// Public API for interacting with a joined group.
///
/// This is the main interface that allows users to issue commands to the group
/// and query its state. The internal state of the group is managed by a
/// long-running worker loop that runs in the background and is associated with
/// the `GroupId` of this group.
pub struct Group<M: StateMachine> {
	state: Arc<WorkerState>,

	/// A reference to the global map of all active groups in the local node,
	/// which is used when the group is dropped to remove itself from the map.
	groups: Arc<DashMap<GroupId, Arc<WorkerState>>>,

	#[doc(hidden)]
	_p: PhantomData<M>,
}

// Public APIs for querying the status of the group
impl<M: StateMachine> Group<M> {
	/// Returns the unique identifier of this group, which is derived from the
	/// group key and the hash of its configuration. See [`GroupId`] for more
	/// details on how the group id is derived.
	pub fn id(&self) -> &GroupId {
		self.state.group_id()
	}

	/// Returns `true` if the local node is currently the leader of this group.
	pub fn is_leader(&self) -> bool {
		self.state.when.current_leader() == Some(self.state.local_id())
	}

	/// Returns the `PeerId` of the current leader of this group, or `None` if no
	/// leader has been elected yet or the last known leader is no longer
	/// responsive.
	pub fn leader(&self) -> Option<PeerId> {
		self.state.when.current_leader()
	}

	/// Returns the list of all group members that are currently bonded and
	/// connected to the local node.
	pub fn bonds(&self) -> Bonds {
		self.state.bonds.clone()
	}

	/// Returns the configuration settings of this group.
	///
	/// All consensus-relevant parameters will be identical on all members of the
	/// group, as they are used to derive the group id.
	pub fn config(&self) -> &GroupConfig {
		&self.state.config
	}

	/// Returns a reference to the [`When`] event emitter for this group, which
	/// can be used to await changes to the group's state, such as leadership
	/// changes.
	pub fn when(&self) -> &When {
		&self.state.when
	}

	/// Returns the index of the latest command that has been committed to the
	/// group's state machine.
	pub fn committed(&self) -> Index {
		self.state.when.current_committed()
	}

	/// Returns the current log position of the local node, which may be ahead of
	/// the committed index if there are uncommitted log entries.
	pub fn log_position(&self) -> Cursor {
		self.state.when.current_log_pos()
	}
}

// Public APIs for interacting the the replicated state machine of the group.
impl<M: StateMachine> Group<M> {
	/// Issues a command to the group, which will be replicated to all voting
	/// followers and committed to the group's state machine once a quorum of
	/// followers have acknowledged the command.
	///
	/// If the local node is the leader, this method's returned future will
	/// resolve once the command has been replicated to a quorum of followers and
	/// committed to the state machine.
	///
	/// If the local node is a follower, this method will forward the command
	/// to the current leader for processing, and the returned future will resolve
	/// once the leader has replicated the command to a quorum of followers and
	/// committed it to the state machine.
	///
	/// If the local node is offline this method will return an error that carries
	/// the unsent command.
	///
	/// Consecutive calls to this method are guaranteed to be processed in the
	/// order they were issued.
	pub async fn execute(
		&self,
		command: M::Command,
	) -> Result<Index, CommandError<M>> {
		self
			.execute_many([command])
			.await
			.map(|range| *range.start())
	}

	/// Issues a series of commands to the group, which will be replicated to all
	/// voting followers and committed to the group's state machine once a quorum
	/// of followers have acknowledged the commands.
	///
	/// If the local node is the leader, this method's returned future will
	/// resolve once the commands have been replicated to a quorum of followers
	/// and committed to the state machine.
	///
	/// If the local node is a follower, this method will forward the commands
	/// to the current leader for processing, and the returned future will resolve
	/// once the leader has replicated the commands to a quorum of followers and
	/// committed them to the state machine.
	///
	/// If the local node is offline this method will return an error that carries
	/// the unsent commands.
	///
	/// Consecutive calls to this method are guaranteed to be processed in the
	/// order they were issued.
	pub async fn execute_many(
		&self,
		commands: impl IntoIterator<Item = M::Command>,
	) -> Result<IndexRange, CommandError<M>> {
		let assigned_ix = self.feed_many(commands).await?;
		self.when().committed().reaches(assigned_ix.clone()).await;
		Ok(assigned_ix)
	}

	/// Sends a command to the group leader without waiting for it to be committed
	/// to the state machine. The returned future will resolve once the command
	/// has been sent to the leader and the leader has acknowledged receipt of the
	/// command and assigned it an index, but it does not guarantee that the
	/// command has been committed to the state.
	pub async fn feed(
		&self,
		command: M::Command,
	) -> Result<Index, CommandError<M>> {
		self.feed_many([command]).await.map(|range| *range.start())
	}

	/// Sends a series of commands to the group leader without waiting for them to
	/// be committed to the state machine. The returned future will resolve once
	/// the commands have been sent to the leader and the leader has acknowledged
	/// receipt of the commands and assigned them indices but does not guarantee
	/// that the commands have been committed to the state.
	pub async fn feed_many(
		&self,
		commands: impl IntoIterator<Item = M::Command>,
	) -> Result<IndexRange, CommandError<M>> {
		#[allow(clippy::missing_panics_doc)]
		let sender = self
			.state
			.raft_cmd_tx
			.downcast_ref::<UnboundedSender<WorkerRaftCommand<M>>>()
			.expect("invalid raft_tx type. this is a bug.");

		let commands: Vec<_> = commands.into_iter().collect();

		if commands.is_empty() {
			return Err(CommandError::NoCommands);
		}

		let (result_tx, result_rx) = oneshot::channel();
		if let Err(SendError(WorkerRaftCommand::Feed(_, _))) =
			sender.send(WorkerRaftCommand::Feed(commands, result_tx))
		{
			return Err(CommandError::GroupTerminated);
		}

		match result_rx.await {
			Ok(Ok(index_range)) => Ok(index_range),
			Ok(Err(e)) => Err(e), // command processing error (e.g. not leader)
			Err(_) => Err(CommandError::GroupTerminated), // oneshot RecvError
		}
	}

	/// Queries the current state of the group's state machine at the last applied
	/// command.
	///
	/// If `consistency` is set to `Weak`, the query will be processed by the
	/// local node without any guarantee of consistency with the current state of
	/// the group. This is the fastest way to process a query, but it may return
	/// stale or inconsistent data.
	///
	/// If `consistency` is set to `Strong`, the query will be forwarded to the
	/// current leader of the group for processing, which guarantees that the
	/// result is consistent with the current state of the group. However, this
	/// may introduce additional latency if the local node is not the leader.
	pub async fn query(
		&self,
		query: M::Query,
		consistency: Consistency,
	) -> Result<CommittedQueryResult<M>, QueryError<M>> {
		#[allow(clippy::missing_panics_doc)]
		let sender = self
			.state
			.raft_cmd_tx
			.downcast_ref::<UnboundedSender<WorkerRaftCommand<M>>>()
			.expect("invalid raft_tx type. this is a bug.");

		let (result_tx, result_rx) = oneshot::channel();
		if let Err(SendError(WorkerRaftCommand::Query(_, _, _))) =
			sender.send(WorkerRaftCommand::Query(query, consistency, result_tx))
		{
			return Err(QueryError::GroupTerminated);
		}

		match result_rx.await {
			Ok(Ok(result)) => Ok(result),
			Ok(Err(e)) => Err(e), // query processing error
			Err(_) => Err(QueryError::GroupTerminated), // oneshot RecvError
		}
	}
}

impl<M: StateMachine> Drop for Group<M> {
	fn drop(&mut self) {
		self.state.bonds.notify_departure();
		self.state.cancel.cancel();
		self.groups.remove(self.id());
	}
}

impl<M: StateMachine> CommittedQueryResult<M> {
	/// Consumes the `CommittedQueryResult` and returns the inner query result.
	pub fn into(self) -> M::QueryResult {
		self.result
	}

	/// Returns a reference to the query result.
	pub const fn result(&self) -> &M::QueryResult {
		&self.result
	}

	/// Returns the index of the committed command at which the query was
	/// processed.
	pub const fn state_position(&self) -> Index {
		self.at_position
	}
}

impl<M: StateMachine> PartialEq<M::QueryResult> for CommittedQueryResult<M>
where
	M::QueryResult: PartialEq,
{
	fn eq(&self, other: &M::QueryResult) -> bool {
		&self.result == other
	}
}

impl<M: StateMachine> core::fmt::Display for CommittedQueryResult<M>
where
	M::QueryResult: core::fmt::Display,
{
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}", &self.result)
	}
}

// Internal APIs
impl<M: StateMachine> Group<M> {
	pub(super) const fn new(
		state: Arc<WorkerState>,
		groups: Arc<DashMap<GroupId, Arc<WorkerState>>>,
	) -> Self {
		Self {
			state,
			groups,
			_p: PhantomData,
		}
	}
}

impl<M: StateMachine> fmt::Display for Group<M> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "Group({})", Short(self.id()))
	}
}
