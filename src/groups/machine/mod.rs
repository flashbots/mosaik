//! Replicated State Machine (RSM) module for the Raft consensus algorithm. This
//! module defines the `StateMachine` trait, which represents the
//! application-specific logic of a Raft group.
//!
//! Each group has a state machine that processes commands and queries, and the
//! log module maintains a replicated log of commands that are applied to the
//! state machine.

use {
	crate::{
		Datum,
		groups::{Cursor, Term},
		primitives::UniqueId,
	},
	serde::{Serialize, de::DeserializeOwned},
};

mod noop;
mod sync;

#[doc(hidden)]
pub use noop::NoOp;
// Public API traits for user-provided state machine implementations.
pub use sync::*;

/// This trait defines the replicated state machine (RSM) that is used by the
/// Raft log. Each group has a state machine that represents the
/// application-specific logic of the group.
pub trait StateMachine: Sized + Send + Sync + 'static {
	/// The type of commands that are applied to the state machine and replicated
	/// in the log. Commands represent state transitions and mutate the state
	/// machine. They are sent to the leader by clients and replicated to
	/// followers via the log.
	type Command: Command;

	/// The type of queries that can be executed against the state machine.
	/// Queries are read-only operations that do not mutate the state machine.
	/// They can be sent to any node in the group (leader or followers) and are
	/// not replicated in the log. They are used to read the current state of the
	/// state machine without modifying it.
	type Query: Query;

	/// The type of results returned by executing queries against the state
	/// machine. This type is returned by the `query` method when a query is
	/// executed by external clients.
	type QueryResult: QueryResult;

	/// The type responsible for implementing the state synchronization (catch-up)
	/// process for followers that are not up to date with the committed group
	/// state.
	///
	/// When writing your own state machine use
	/// [`LogReplaySync`](super::LogReplaySync) as the initial state sync
	/// implementation, which works with any state machine as a starting point.
	type StateSync: StateSync<Machine = Self>;

	/// A unique identifier for the state machine type and settings. This value is
	/// part of the group id derivation and must be identical for all members of
	/// the same group. Any difference in this value will render a different
	/// group id and will prevent peers from joining the same group.
	///
	/// This value should be derived from the state machine implementation type
	/// and any relevant init parameters, such that different state machine
	/// implementations or configurations yield different ids. This is used to
	/// prevent peers with incompatible state machines from joining the same group
	/// and causing undefined behavior.
	fn signature(&self) -> UniqueId;

	/// Applies a command to the state machine, mutating its state. This method is
	/// called by the log when a command is committed (replicated to a majority).
	fn apply(&mut self, command: Self::Command, ctx: &dyn ApplyContext);

	/// Optionally allows state machine implementations to optimize the
	/// application of a batch of commands. By default, this method applies
	/// commands one by one using the `apply` method, but state machines that
	/// can optimize batch processing can override this method.
	#[inline]
	fn apply_batch(
		&mut self,
		commands: impl IntoIterator<Item = Self::Command>,
		ctx: &dyn ApplyContext,
	) {
		for command in commands {
			self.apply(command, ctx);
		}
	}

	/// Executes a query against the state machine, returning a result. This
	/// method is called when an external client sends a query to any node in the
	/// group. The query is executed against the current state of the state
	/// machine and returns a result without modifying the state.
	fn query(&self, query: Self::Query) -> Self::QueryResult;

	/// Returns a new instance of the state synchronization implementation that is
	/// used to synchronize lagging followers with the current state of the group.
	///
	/// When writing your own state machine use
	/// [`LogReplaySync`](super::LogReplaySync) as the initial state sync
	/// implementation, which works with any state machine as a starting point.
	fn state_sync(&self) -> Self::StateSync;

	/// Returns the leadership preference for this state machine instance. This
	/// controls whether the node running this state machine will seek to become
	/// a leader, avoid leadership, or never assume leadership at all.
	///
	/// This is a per-node preference that does not affect the group identity —
	/// different nodes in the same group can have different leadership
	/// preferences. The preference is enforced by the Raft election logic:
	///
	/// - [`LeadershipPreference::Normal`] — default Raft behavior, the node
	///   participates in elections as a candidate.
	/// - [`LeadershipPreference::Reluctant`] — the node uses longer election
	///   timeouts, reducing its likelihood of becoming leader, but can still be
	///   elected if no other candidate is available.
	/// - [`LeadershipPreference::Observer`] — the node never self-nominates as a
	///   candidate. It participates in voting and log replication as a full
	///   member of the group, but will never initiate an election.
	#[inline]
	fn leadership_preference(&self) -> LeadershipPreference {
		LeadershipPreference::Normal
	}
}

/// Contextual information provided to the state machine when applying commands.
///
/// This trait does not offer any information that is non-deterministic or that
/// can diverge between different nodes in the group, so it is safe to use for
/// deterministic state machines.
pub trait ApplyContext {
	/// The index and term of the last committed log entry that has been applied
	/// by the state machine before applying the current batch of commands.
	///
	/// This can be used by the state machine to determine the current position of
	/// the log when it is applying new commands.
	///
	/// If the state machine implements its own `apply_batch` method, it needs to
	/// compute the index of each command in the batch by itself using the number
	/// of commands in the batch and the index of the last committed command
	/// returned by this method.
	fn committed(&self) -> Cursor;

	/// The index and term of the last log entry in the log.
	fn log_position(&self) -> Cursor;

	/// The term of the commands being applied.
	fn current_term(&self) -> Term;
}

pub trait StateMachineMessage:
	Clone + Send + Sync + Serialize + DeserializeOwned + 'static
{
}

impl<T> StateMachineMessage for T where
	T: Clone + Send + Sync + Serialize + DeserializeOwned + 'static
{
}

pub trait Command: Datum + Sync + Clone {}
impl<T> Command for T where T: Datum + Sync + Clone {}

pub trait Query: Datum + Sync + Clone {}
impl<T> Query for T where T: Datum + Sync + Clone {}

pub trait QueryResult: Datum + Sync + Clone {}
impl<T> QueryResult for T where T: Datum + Sync + Clone {}

/// Describes a node's preference for assuming leadership within a group.
///
/// This preference is per-node and does not affect the group identity.
/// Different nodes in the same group can have different leadership preferences.
/// The preference is enforced by the Raft election logic at the follower level:
/// observers never transition to candidate state, and reluctant nodes use
/// longer election timeouts to reduce their likelihood of winning elections.
///
/// All preference levels participate fully in voting and log replication. The
/// distinction is solely about whether and how eagerly the node self-nominates
/// as a candidate during leader elections.
///
/// # Safety Considerations
///
/// - **Observer nodes still vote.** They are full members of the group and
///   count toward quorum. They simply never initiate elections.
/// - **Liveness.** If all nodes in a group are observers, no leader can ever be
///   elected and the group will be unable to make progress. Ensure that at
///   least one node in every group has `Normal` or `Reluctant` preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LeadershipPreference {
	/// Default Raft behavior. The node participates in elections as a
	/// candidate with standard election timeouts.
	#[default]
	Normal,

	/// The node uses longer election timeouts, reducing its likelihood of
	/// becoming leader. It can still be elected if no other candidate is
	/// available (e.g., during network partitions or when all preferred
	/// leaders are down). The factor controls the timeout multiplier
	/// (e.g., a factor of 3 triples the election timeout).
	Reluctant {
		/// The multiplier applied to the election timeout and bootstrap
		/// delay. Must be greater than 1 for the deprioritization to have
		/// any effect.
		factor: u32,
	},

	/// The node never self-nominates as a candidate. It participates in
	/// voting and log replication as a full member of the group, but will
	/// never initiate an election. This provides a hard guarantee that the
	/// node will not become leader, unlike `Reluctant` which is
	/// probabilistic.
	Observer,
}

impl LeadershipPreference {
	/// Creates a reluctant preference with the default factor of 3.
	pub const fn reluctant() -> Self {
		Self::Reluctant { factor: 3 }
	}
}
