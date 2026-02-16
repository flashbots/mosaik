use crate::{
	groups::{StateMachine, replay::LogReplaySync},
	primitives::UniqueId,
};

/// A no-op state machine that does nothing and can be used for testing or
/// agreeing on who's the leader without any application-level logic.
#[derive(Debug, Default)]
pub struct NoOp;

impl StateMachine for NoOp {
	type Command = ();
	type Query = ();
	type QueryResult = ();
	type StateSync = LogReplaySync<Self>;

	fn signature(&self) -> UniqueId {
		// noop has no configs so we can use a constant signature.
		UniqueId::from("mosaik_noop_state_machine")
	}

	fn apply(&mut self, (): Self::Command) {}

	fn query(&self, (): Self::Query) -> Self::QueryResult {}

	fn sync_factory(&self) -> Self::StateSync {
		LogReplaySync::default()
	}
}
