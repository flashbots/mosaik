mod driver;
mod rsm;
mod storage;

/// Raft term, increases monotonically with every new leader election.
pub type Term = u64;

/// Raft log index increases monotonically with every new log entry.
pub type Index = u64;

// Public API exports
pub use {
	driver::Driver,
	rsm::{Command, Query, QueryResult, StateMachine},
	storage::Storage,
};
