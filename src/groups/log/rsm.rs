//! Replicated State Machine (RSM) module for the Raft consensus algorithm. This
//! module defines the `StateMachine` trait, which represents the
//! application-specific logic of a Raft group.
//!
//! Each group has a state machine that processes commands and queries, and the
//! log module maintains a replicated log of commands that are applied to the
//! state machine. The RSM is

use {
	crate::primitives::UniqueId,
	core::{fmt::Debug, num::NonZero},
	serde::{Serialize, de::DeserializeOwned},
};

/// This trait defines the replicated state machine (RSM) that is used by the
/// Raft log. Each group has a state machine that represents the
/// application-specific logic of the group.
pub trait StateMachine: Send + Sync + Unpin + 'static {
	/// A unique identifier for the state machine type. This value is part of the
	/// group id derivation and must be identical for all members of the same
	/// group. Any difference in this value will render a different group id and
	/// will prevent peers from joining the same group.
	const ID: UniqueId;

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

	/// Reset to initial state. Called when the log is truncated
	/// below the commit index due to rival leader reconciliation.
	fn reset(&mut self);

	/// Applies a command to the state machine, mutating its state. This method is
	/// called by the log when a command is committed (replicated to a majority).
	fn apply(&mut self, command: Self::Command);

	/// Executes a query against the state machine, returning a result. This
	/// method is called when an external client sends a query to any node in the
	/// group. The query is executed against the current state of the state
	/// machine and returns a result without modifying the state.
	fn query(&self, query: Self::Query) -> Self::QueryResult;

	/// The recommended maximum number of commands to include in a single
	/// follower-catchup response.
	///
	/// This value is used when followers lag behind the leader and need to catch
	/// up by receiving historical commands from other group members.
	///
	/// As the author of the state machine implementation, try to keep this value
	/// so that individual chunks are in the 1-2 MB range.
	fn catchup_chunk_size(&self) -> NonZero<u64> {
		NonZero::new(1000).unwrap()
	}
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
