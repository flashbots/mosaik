use crate::{groups::StateMachine, primitives::UniqueId, unique_id};

/// A no-op state machine that does nothing and can be used for testing or
/// agreeing on who's the leader without any application-level logic.
#[derive(Debug, Default)]
pub struct NoOp;

impl StateMachine for NoOp {
	type Command = ();
	type Query = ();
	type QueryResult = ();

	const ID: UniqueId = unique_id!(
		"0000000000000000000000000000000000000000000000000000000000000001"
	);

	fn apply(&mut self, (): Self::Command) {}

	fn query(&self, (): Self::Query) -> Self::QueryResult {}
}
