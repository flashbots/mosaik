use crate::groups::log::{rsm::StateMachine, storage::Storage};

pub struct Driver<S, M>
where
	S: Storage<M::Command>,
	M: StateMachine,
{
	storage: S,
	machine: M,
}

impl<S, M> Driver<S, M>
where
	S: Storage<M::Command>,
	M: StateMachine,
{
	pub const fn new(storage: S, machine: M) -> Self {
		Self { storage, machine }
	}

	pub fn query(&self, query: M::Query) -> M::QueryResult {
		self.machine.query(query)
	}
}
