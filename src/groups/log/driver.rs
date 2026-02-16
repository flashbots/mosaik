use crate::groups::{Cursor, Index, StateMachine, Term, log::storage::Storage};

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
			committed: Index::zero(),
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
	pub fn last(&self) -> Cursor {
		self.storage.last().unwrap_or_default()
	}

	/// Returns the index of the latest committed log entry that has
	/// received a majority of votes from followers.
	pub const fn committed(&self) -> Index {
		self.committed
	}

	/// Returns the term of the entry at the given index, or None if no entry
	/// exists at that index. Log entries are indexed starting from 1, so
	/// `term_at(0)` always returns `Some(0)`.
	pub fn term_at(&self, index: Index) -> Option<Term> {
		if index.is_zero() {
			return Some(Term::zero());
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
	///
	/// Returns the index of the latest committed entry after this operation,
	/// which may be less than the given index goes beyond the end of the log.
	pub fn commit_up_to(&mut self, index: Index) -> Index {
		let committed = self.committed;
		let range = committed.next()..=index;
		let commands = self.storage.get_range(&range);
		let commands_count = commands.len() as u64;

		self
			.machine
			.apply_batch(commands.into_iter().map(|(_, _, cmd)| cmd));
		self.committed = committed + commands_count;

		self.committed
	}

	/// Returns a reference to the state machine.
	pub const fn machine(&self) -> &M {
		&self.machine
	}

	/// Returns a mutable reference to the state machine.
	pub const fn machine_mut(&mut self) -> &mut M {
		&mut self.machine
	}

	/// Returns a reference to the raw log storage.
	pub const fn storage(&self) -> &S {
		&self.storage
	}

	/// Returns a mutable reference to the raw log storage.
	pub const fn storage_mut(&mut self) -> &mut S {
		&mut self.storage
	}
}
