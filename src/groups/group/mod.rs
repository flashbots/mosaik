use {
	crate::groups::{Bonds, GroupId, StateMachine},
	state::WorkerState,
	std::sync::Arc,
};

mod builder;
mod error;

pub(in crate::groups) mod state;
pub(super) mod worker;

pub use {
	builder::{
		GroupBuilder,
		IntervalsConfig,
		IntervalsConfigBuilder,
		IntervalsConfigBuilderError,
	},
	error::Error,
};

/// Query consistency levels for group state machine queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consistency {
	/// The query will be processed by the local node without any guarantee of
	/// consistency with the current state of the group. This is the fastest way
	/// to process a query, but it may return stale or inconsistent data.
	Weak,

	/// The query will be forwarded to the current leader of the group for
	/// processing, which guarantees that the result is consistent with the
	/// current state of the group. However, this may introduce additional
	/// latency if the local node is not the leader or if there are network
	/// issues.
	Strong,
}

/// Public API for interacting with a joined group.
///
/// This is the main interface that allows users to issue commands to the group
/// and query its state. The internal state of the group is managed by a
/// long-running worker loop that runs in the background and is associated with
/// the `GroupId` of this group.
pub struct Group<M: StateMachine> {
	handle: Arc<WorkerState>,
	_p: std::marker::PhantomData<M>,
}

// Public APIs for querying the status of the group
impl<M: StateMachine> Group<M> {
	/// Returns the unique identifier of this group, which is derived from the
	/// group key and the hash of its configuration.
	pub fn group_id(&self) -> &GroupId {
		self.handle.group_id()
	}

	/// Returns `true` if the local node is currently the leader of this group.
	pub fn is_leader(&self) -> bool {
		todo!()
	}

	/// Returns the list of all group members that are currently bonded and
	/// connected to the local node.
	pub fn bonds(&self) -> Bonds {
		self.handle.bonds.clone()
	}
}

// Public APIs for interacting the the replicated state machine of the group.
impl<M: StateMachine> Group<M> {
	/// Issues a command to the group, which will be replicated to all members and
	/// eventually applied to the group's state machine. This function will return
	/// an error if the command could not be applied for any reason.
	///
	/// If the local node is not the leader of the group, this function will
	/// forward the command to the current leader for processing.
	pub async fn command(&self, _command: M::Command) -> Result<(), Error> {
		async { todo!() }.await
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
		_query: M::Query,
		_consistency: Consistency,
	) -> Result<M::QueryResult, Error> {
		async { todo!() }.await
	}
}
