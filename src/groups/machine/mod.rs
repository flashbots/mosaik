//! Replicated State Machine (RSM) module for the Raft consensus algorithm. This
//! module defines the `StateMachine` trait, which represents the
//! application-specific logic of a Raft group.
//!
//! Each group has a state machine that processes commands and queries, and the
//! log module maintains a replicated log of commands that are applied to the
//! state machine. The RSM is

use {
	crate::primitives::UniqueId,
	core::fmt::Debug,
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
pub trait StateMachine: Send + Sync + Unpin + 'static {
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
	fn apply(&mut self, command: Self::Command);

	/// Executes a query against the state machine, returning a result. This
	/// method is called when an external client sends a query to any node in the
	/// group. The query is executed against the current state of the state
	/// machine and returns a result without modifying the state.
	fn query(&self, query: Self::Query) -> Self::QueryResult;

	/// Returns a new instance of the state synchronization implementation that is
	/// used to synchronize lagging followers with the current state of the group.
	fn sync_factory(&self) -> Self::StateSync;
}

pub trait Command:
	Debug + Clone + Send + Sync + Unpin + Serialize + DeserializeOwned + 'static
{
}

impl<T> Command for T where
	T: Debug
		+ Clone
		+ Send
		+ Sync
		+ Unpin
		+ Serialize
		+ DeserializeOwned
		+ 'static
{
}

pub trait Query:
	Debug + Clone + Send + Sync + Unpin + Serialize + DeserializeOwned + 'static
{
}

impl<T> Query for T where
	T: Debug
		+ Clone
		+ Send
		+ Sync
		+ Unpin
		+ Serialize
		+ DeserializeOwned
		+ 'static
{
}

pub trait QueryResult:
	Debug + Clone + Send + Sync + Unpin + Serialize + DeserializeOwned + 'static
{
}

impl<T> QueryResult for T where
	T: Debug
		+ Clone
		+ Send
		+ Sync
		+ Unpin
		+ Serialize
		+ DeserializeOwned
		+ 'static
{
}
