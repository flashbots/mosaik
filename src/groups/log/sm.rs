/// This trait defines the replicated state machine (RSM) that is used by the
/// Raft log. Each group has a state machine that represents the
/// application-specific logic of the group.
pub trait StateMachine {
	/// The type of commands that are applied to the state machine and replicated
	/// in the log. Commands represent state transitions and mutate the state
	/// machine. They are sent to the leader by clients and replicated to
	/// followers via the log.
	type Command;

	/// The type of queries that can be executed against the state machine.
	/// Queries are read-only operations that do not mutate the state machine.
	/// They can be sent to any node in the group (leader or followers) and are
	/// not replicated in the log. They are used to read the current state of the
	/// state machine without modifying it.
	type Query;
}
