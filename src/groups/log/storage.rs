use {
	crate::groups::{
		Command,
		Cursor,
		log::{Index, Term},
	},
	core::ops::RangeInclusive,
};

/// Defines the storage interface for the Raft log.
///
/// This trait abstracts over the underlying storage mechanism used to persist
/// log entries. The log module uses this trait to append new log entries,
/// retrieve existing entries, and truncate the log when necessary (e.g., when a
/// follower falls behind and needs to discard old entries).
///
/// Entries in the log are indexed starting from 1.
pub trait Storage<C: Command>: Send + Sync + Unpin + 'static {
	/// Appends a new log entry to the end of the log and returns the index of
	/// the newly appended entry.
	fn append(&mut self, command: C, term: Term) -> Index;

	/// Returns the range of available log indices in the store.
	/// This is used to determine which log entries can be retrieved in their
	/// original form and which ones have been truncated and compacted.
	///
	/// This is used during the log synchronization process when a lagging
	/// follower needs to catch up with the leader.
	fn available(&self) -> RangeInclusive<Index>;

	/// Retrieves the log entry at the specified index, if it exists. Returns
	/// `None` if the index is out of bounds (e.g., if it has been truncated or if
	/// it has not been appended yet).
	fn get(&self, index: Index) -> Option<(C, Term)>;

	/// Retrieves a range of log entries from the log, starting from `start`
	/// and ending at `end` (inclusive). Returns an iterator over the entries in
	/// the specified range. If any index in the range is out of bounds, it is
	/// skipped and not included in the returned iterator.
	fn get_range(&self, range: &RangeInclusive<Index>) -> Vec<(Term, Index, C)>;

	/// Removes all log entries starting from the specified index (inclusive) to
	/// the end of the log. This is used when a follower falls behind and needs to
	/// discard old entries.
	fn truncate(&mut self, at: Index);

	/// Returns the index and term of the last log entry.
	/// Returns `None` if the log is empty.
	fn last(&self) -> Option<Cursor>;
}

/// An in-memory implementation of the `Storage` trait replicated log storage in
/// groups. This storage implementation is compatible with arbitrary command
/// types.
#[derive(Debug)]
pub struct InMemoryLogStore<C: Command> {
	entries: Vec<(C, Term)>,
}

impl<C: Command> Default for InMemoryLogStore<C> {
	fn default() -> Self {
		Self {
			entries: Vec::new(),
		}
	}
}

impl<C: Command> Storage<C> for InMemoryLogStore<C> {
	fn append(&mut self, command: C, term: Term) -> Index {
		self.entries.push((command, term));
		(self.entries.len() as u64).into()
	}

	fn get(&self, index: Index) -> Option<(C, Term)> {
		if index.is_zero() {
			return None;
		}
		let index: usize = index.prev().into();
		self.entries.get(index).cloned()
	}

	fn available(&self) -> std::ops::RangeInclusive<Index> {
		Index::zero()..=(self.entries.len() as u64).into()
	}

	fn get_range(&self, range: &RangeInclusive<Index>) -> Vec<(Term, Index, C)> {
		let start: usize = range.start().prev().into();
		let end: usize = range.end().prev().into();

		self
			.entries
			.iter()
			.enumerate()
			.skip(start)
			.take(end - start + 1)
			.map(move |(i, (cmd, term))| (*term, (start + i + 1).into(), cmd.clone()))
			.collect()
	}

	fn truncate(&mut self, at: Index) {
		if at <= Index(1) {
			self.entries.clear();
		} else {
			self.entries.truncate((at.prev()).into());
		}
	}

	fn last(&self) -> Option<Cursor> {
		if self.entries.is_empty() {
			None
		} else {
			let (_, term) = self.entries.last().unwrap();
			Some(Cursor(*term, (self.entries.len() as u64).into()))
		}
	}
}
