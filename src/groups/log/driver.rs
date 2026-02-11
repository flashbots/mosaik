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
	/// The underlying log entries storage.
	storage: S,

	/// The state machine that applies committed log entries and responds to
	/// queries.
	machine: M,

	/// Index of the latest committed log entry that has been applied to the
	/// state
	committed: Index,
}

impl<S, M> Driver<S, M>
where
	S: Storage<M::Command>,
	M: StateMachine,
{
	pub const fn new(storage: S, machine: M) -> Self {
		Self {
			storage,
			machine,
			committed: 0,
		}
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

	/// Retrieves the entry at the given index.
	pub fn get(&self, index: Index) -> Option<(M::Command, Term)> {
		self.storage.get(index)
	}

	/// Returns the term of the entry at the given index, or None if no entry
	/// exists at that index. Log entries are indexed starting from 1, so
	/// `term_at(0)` always returns `Some(0)`.
	pub fn term_at(&self, index: Index) -> Option<Term> {
		if index == 0 {
			return Some(0);
		}

		self.storage.get(index).map(|(_, term)| term)
	}

	/// Truncates the log from `at` onward (inclusive).
	pub fn truncate(&mut self, at: Index) {
		self.storage.truncate(at);
	}

	/// Appends a new entry.
	pub fn append(&mut self, command: M::Command, term: Term) -> Index {
		self.storage.append(command, term)
	}

	/// Commits log entries up to the given index and applies them to the state
	/// machine. This should only be called with an index that has been replicated
	/// to a majority of followers, which is guaranteed by the Raft leader before
	/// calling this method.
	pub fn commit_up_to(&mut self, index: Index) {
		for i in self.committed + 1..=index {
			if let Some((command, _)) = self.storage.get(i) {
				self.machine.apply(command);
			}
		}

		self.committed = index;
	}
}
