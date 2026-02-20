use {
	crate::groups::{Command, Cursor, Index, Term},
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
pub trait Storage<C: Command>: Send + 'static {
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
	fn last(&self) -> Cursor;

	/// Returns the term of the log entry at the specified index, if it exists.
	/// Returns `None` if the index is out of bounds (e.g., if it has been
	/// truncated or if it has not been appended yet).
	fn term_at(&self, index: Index) -> Option<Term> {
		self.get(index).map(|(_, term)| term)
	}

	/// Removes all log entries up to the specified index (inclusive) from the
	/// log.
	///
	/// This is used to prune old log entries that are no longer needed for
	/// state synchronization of lagging followers, while ensuring that the
	/// latest committed entry is never pruned.
	///
	/// Not all state synchronization implementations support logs pruning.
	/// After pruning entries should still maintain their original indices, so
	/// that the index of the latest committed entry is never affected.
	fn prune_prefix(&mut self, up_to: Index);
}

/// An in-memory implementation of the `Storage` trait replicated log storage in
/// groups. This storage implementation is compatible with arbitrary command
/// types.
#[derive(Debug)]
pub struct InMemoryLogStore<C: Command> {
	entries: Vec<(C, Term)>,
	offset: u64,
}

impl<C: Command> Default for InMemoryLogStore<C> {
	fn default() -> Self {
		Self {
			entries: Vec::new(),
			offset: 0,
		}
	}
}

impl<C: Command> Storage<C> for InMemoryLogStore<C> {
	fn append(&mut self, command: C, term: Term) -> Index {
		self.entries.push((command, term));
		(self.offset + self.entries.len() as u64).into()
	}

	fn get(&self, index: Index) -> Option<(C, Term)> {
		if index.is_zero() || index.0 <= self.offset {
			return None;
		}
		let physical = (index.0 - self.offset - 1) as usize;
		self.entries.get(physical).cloned()
	}

	fn available(&self) -> std::ops::RangeInclusive<Index> {
		Index(self.offset)..=Index(self.offset + self.entries.len() as u64)
	}

	fn get_range(&self, range: &RangeInclusive<Index>) -> Vec<(Term, Index, C)> {
		// Clamp the requested range to entries that are actually available
		// (i.e. not pruned and not beyond the end of the log).
		let first_available = self.offset + 1;
		let start = range.start().0.max(first_available);
		let last_logical = self.offset + self.entries.len() as u64;
		let end = range.end().0.min(last_logical);

		if start > end {
			return Vec::new();
		}

		let phys_start = (start - self.offset - 1) as usize;
		let phys_end = (end - self.offset - 1) as usize;

		self
			.entries
			.iter()
			.enumerate()
			.skip(phys_start)
			.take(phys_end - phys_start + 1)
			.map(move |(i, (cmd, term))| {
				let logical_index = self.offset + 1 + i as u64;
				(*term, Index(logical_index), cmd.clone())
			})
			.collect()
	}

	fn truncate(&mut self, at: Index) {
		if at.0 <= self.offset {
			// Truncation point is at or before the pruned prefix; clear
			// everything that remains.
			self.entries.clear();
		} else {
			let physical = (at.0 - self.offset - 1) as usize;
			self.entries.truncate(physical);
		}
	}

	fn last(&self) -> Cursor {
		if self.entries.is_empty() {
			Cursor::default()
		} else {
			let (_, term) = self.entries.last().unwrap();
			let logical_index = self.offset + self.entries.len() as u64;
			Cursor(*term, Index(logical_index))
		}
	}

	fn prune_prefix(&mut self, up_to: Index) {
		if up_to.0 <= self.offset {
			// Already pruned past this point.
			return;
		}

		let to_drain = ((up_to.0 - self.offset) as usize).min(self.entries.len());
		self.entries.drain(..to_drain);
		self.offset += to_drain as u64;
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn term(n: u64) -> Term {
		Term(n)
	}

	fn populate(store: &mut InMemoryLogStore<u64>, n: u64) {
		for i in 1..=n {
			store.append(i * 10, term(1));
		}
	}

	#[test]
	fn prune_prefix_basic() {
		let mut store = InMemoryLogStore::<u64>::default();
		populate(&mut store, 5); // entries at indices 1..=5

		store.prune_prefix(Index(2)); // prune entries 1 and 2

		// available should reflect the pruned prefix
		let avail = store.available();
		assert_eq!(*avail.start(), Index(2));
		assert_eq!(*avail.end(), Index(5));

		// pruned entries return None
		assert!(store.get(Index(1)).is_none());
		assert!(store.get(Index(2)).is_none());

		// remaining entries are still accessible with original indices
		assert_eq!(store.get(Index(3)), Some((30, term(1))));
		assert_eq!(store.get(Index(4)), Some((40, term(1))));
		assert_eq!(store.get(Index(5)), Some((50, term(1))));
	}

	#[test]
	fn prune_prefix_last_unchanged() {
		let mut store = InMemoryLogStore::<u64>::default();
		populate(&mut store, 5);

		let last_before = store.last();
		store.prune_prefix(Index(3));
		let last_after = store.last();

		// last() should still report the same logical position
		assert_eq!(last_before, last_after);
		assert_eq!(last_after, Cursor(term(1), Index(5)));
	}

	#[test]
	fn prune_prefix_then_append() {
		let mut store = InMemoryLogStore::<u64>::default();
		populate(&mut store, 3);

		store.prune_prefix(Index(2)); // prune 1, 2

		// append should return correct logical index
		let idx = store.append(60, term(2));
		assert_eq!(idx, Index(4));

		assert_eq!(store.get(Index(4)), Some((60, term(2))));
		assert_eq!(store.last(), Cursor(term(2), Index(4)));
	}

	#[test]
	fn prune_prefix_get_range() {
		let mut store = InMemoryLogStore::<u64>::default();
		populate(&mut store, 6);

		store.prune_prefix(Index(2));

		// range fully within available entries
		let entries = store.get_range(&(Index(3)..=Index(5)));
		assert_eq!(entries.len(), 3);
		assert_eq!(entries[0], (term(1), Index(3), 30));
		assert_eq!(entries[1], (term(1), Index(4), 40));
		assert_eq!(entries[2], (term(1), Index(5), 50));

		// range overlapping pruned prefix should skip pruned entries
		let entries = store.get_range(&(Index(1)..=Index(4)));
		assert_eq!(entries.len(), 2); // only 3 and 4
		assert_eq!(entries[0], (term(1), Index(3), 30));
		assert_eq!(entries[1], (term(1), Index(4), 40));

		// range entirely in pruned area
		let entries = store.get_range(&(Index(1)..=Index(2)));
		assert!(entries.is_empty());
	}

	#[test]
	fn prune_prefix_then_truncate() {
		let mut store = InMemoryLogStore::<u64>::default();
		populate(&mut store, 6);

		store.prune_prefix(Index(2)); // prune 1, 2
		store.truncate(Index(5)); // truncate from 5 onward → keeps 3, 4

		assert_eq!(store.last(), Cursor(term(1), Index(4)));
		assert!(store.get(Index(5)).is_none());
		assert!(store.get(Index(6)).is_none());
		assert_eq!(store.get(Index(4)), Some((40, term(1))));
	}

	#[test]
	fn prune_prefix_idempotent() {
		let mut store = InMemoryLogStore::<u64>::default();
		populate(&mut store, 5);

		store.prune_prefix(Index(3));
		store.prune_prefix(Index(2)); // already pruned past this
		store.prune_prefix(Index(3)); // same as current offset

		let avail = store.available();
		assert_eq!(*avail.start(), Index(3));
		assert_eq!(*avail.end(), Index(5));
		assert_eq!(store.get(Index(4)), Some((40, term(1))));
	}

	#[test]
	fn prune_prefix_all_entries() {
		let mut store = InMemoryLogStore::<u64>::default();
		populate(&mut store, 3);

		store.prune_prefix(Index(3)); // prune everything

		assert_eq!(store.last(), Cursor::zero());
		let avail = store.available();
		assert_eq!(*avail.start(), Index(3));
		assert_eq!(*avail.end(), Index(3));

		// append after full prune should work correctly
		let idx = store.append(100, term(2));
		assert_eq!(idx, Index(4));
		assert_eq!(store.get(Index(4)), Some((100, term(2))));
	}

	#[test]
	fn prune_prefix_zero_is_noop() {
		let mut store = InMemoryLogStore::<u64>::default();
		populate(&mut store, 3);

		store.prune_prefix(Index(0));

		let avail = store.available();
		assert_eq!(*avail.start(), Index(0));
		assert_eq!(*avail.end(), Index(3));
		assert_eq!(store.get(Index(1)), Some((10, term(1))));
	}

	#[test]
	fn truncate_at_pruned_boundary_clears_remaining() {
		let mut store = InMemoryLogStore::<u64>::default();
		populate(&mut store, 5);

		store.prune_prefix(Index(3)); // prune 1, 2, 3
		store.truncate(Index(2)); // truncation point is within pruned area

		// everything remaining should be cleared
		assert!(store.last().is_zero());
	}
}
