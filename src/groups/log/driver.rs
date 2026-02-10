use crate::groups::{
	Index,
	Term,
	log::{rsm::StateMachine, storage::Storage},
};

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

impl<S, M> Driver<S, M>
where
	S: Storage<M::Command>,
	M: StateMachine,
{
	/// Returns the term and index of the latest committed log entry.
	pub fn last(&self) -> (Term, Index) {
		self.storage.last().unwrap_or((0, 0))
	}

	/// Returns the term and index of the latest committed log entry that has
	/// received a majority of votes from followers.
	pub const fn committed(&self) -> (Term, Index) {
		(0, 0)
	}
}
