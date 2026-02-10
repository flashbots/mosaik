use {
	crate::groups::log::{Index, Term, rsm::Command},
	core::ops::Range,
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

	/// Retrieves the log entry at the specified index, if it exists. Returns
	/// `None` if the index is out of bounds (e.g., if it has been truncated or if
	/// it has not been appended yet).
	fn get(&self, index: Index) -> Option<(C, Term)>;

	/// Retrieves a range of log entries from the log, starting from `start`
	/// and ending at `end` (inclusive). Returns an iterator over the entries in
	/// the specified range. If any index in the range is out of bounds, it is
	/// skipped and not included in the returned iterator.
	fn get_range(
		&self,
		range: Range<Index>,
	) -> impl Iterator<Item = (Term, Index, C)> + '_;

	/// Removes all log entries starting from the specified index (inclusive) to
	/// the end of the log. This is used when a follower falls behind and needs to
	/// discard old entries.
	fn truncate(&mut self, at: Index);

	/// Returns the index and term of the last log entry.
	/// Returns `None` if the log is empty.
	fn last(&self) -> Option<(Term, Index)>;
}
