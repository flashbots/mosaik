use {
	core::fmt::Debug,
	serde::{Deserialize, Serialize},
	tokio::sync::watch,
};

mod sm;

/// Raft term, increases monotonically with every new leader election.
pub type Term = u64;

/// Raft log index increases monotonically with every new log entry.
pub type Index = u64;

/// A single entry in the replicated log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry<C> {
	/// The term when this entry was received by the leader.
	pub term: Term,

	/// The index of this entry in the log (1-indexed).
	pub index: Index,

	/// The command data to be applied to the state machine.
	pub command: C,
}

/// A replicated log of state machine operations using the Raft consensus
/// protocol.
///
/// The log maintains entries that are replicated across all group members.
/// Once an entry is committed (replicated to a majority), it is applied to
/// the state machine via the provided callback.
///
/// The log is generic over the command type `C`, allowing different state
/// machines to use the same log infrastructure.
pub struct Log<C> {
	/// The log entries. Index 0 is a sentinel; entries are 1-indexed.
	entries: Vec<LogEntry<C>>,

	/// Index of highest log entry known to be committed.
	/// Committed entries are safe to apply to the state machine.
	commit_index: watch::Sender<Index>,

	/// Index of highest log entry applied to the state machine.
	/// Invariant: last_applied <= commit_index
	last_applied: watch::Sender<Index>,
}

impl<C> Log<C>
where
	C: Clone + Default,
{
	/// Creates a new empty log.
	pub fn new() -> Self {
		Self {
			// Index 0 is a sentinel; real entries start at index 1
			entries: vec![LogEntry {
				term: 0,
				index: 0,
				command: C::default(),
			}],
			commit_index: watch::Sender::new(0),
			last_applied: watch::Sender::new(0),
		}
	}
}

impl<C> Default for Log<C>
where
	C: Clone + Default,
{
	fn default() -> Self {
		Self::new()
	}
}

impl<C> Log<C>
where
	C: Clone,
{
	/// Returns the index of the last log entry.
	pub fn index(&self) -> Index {
		self.entries.last().map(|e| e.index).unwrap_or(0)
	}

	/// Returns the term of the last log entry.
	pub fn term(&self) -> Term {
		self.entries.last().map(|e| e.term).unwrap_or(0)
	}

	/// Returns the current commit index.
	pub fn committed(&self) -> Index {
		*self.commit_index.borrow()
	}

	/// Returns a watch receiver for commit index updates.
	pub fn committed_watch(&self) -> watch::Receiver<Index> {
		self.commit_index.subscribe()
	}

	/// Returns the index of the last applied entry.
	pub fn applied(&self) -> Index {
		*self.last_applied.borrow()
	}

	/// Returns a watch receiver for last applied index updates.
	pub fn applied_watch(&self) -> watch::Receiver<Index> {
		self.last_applied.subscribe()
	}

	/// Returns the entry at the given index, if it exists.
	pub fn get(&self, index: Index) -> Option<&LogEntry<C>> {
		if index == 0 {
			return None;
		}
		self.entries.get(index as usize)
	}

	/// Returns the term at the given index, or 0 if index is 0 or out of bounds.
	pub fn term_at(&self, index: Index) -> Term {
		self.get(index).map_or(0, |e| e.term)
	}

	/// Appends a new entry to the log (leader only).
	/// Returns the index of the new entry.
	pub fn append(&mut self, term: Term, command: C) -> Index {
		let index = self.index() + 1;
		assert_eq!(index, self.entries.len() as u64);

		self.entries.push(LogEntry {
			term,
			index,
			command,
		});
		index
	}

	/// Handles incoming entries from an `AppendEntries` message.
	///
	/// - `prev_log_index`: Index of log entry immediately preceding new ones
	/// - `prev_log_term`: Term of `prev_log_index` entry
	/// - `entries`: Log entries to store (may be empty for heartbeat)
	///
	/// Returns `true` if the entries were accepted (log matched at
	/// prev_log_index).
	pub fn append_entries(
		&mut self,
		prev_log_index: u64,
		prev_log_term: Term,
		entries: Vec<LogEntry<C>>,
	) -> bool {
		// Check if log contains entry at prev_log_index with matching term
		if prev_log_index > 0 {
			if let Some(entry) = self.get(prev_log_index)
				&& entry.term == prev_log_term
			{
				// match - continue
			} else {
				// no match - reject
				return false;
			}
		}

		// Append new entries, removing any conflicting entries
		for entry in entries {
			let idx = entry.index as usize;

			if idx < self.entries.len() {
				// Entry exists at this index
				if self.entries[idx].term != entry.term {
					// Conflict: delete this entry and all that follow
					self.entries.truncate(idx);
					self.entries.push(entry);
				}
				// Otherwise, entry already exists with same term - skip
			} else {
				// New entry - append
				self.entries.push(entry);
			}
		}

		true
	}

	/// Updates the commit index.
	///
	/// Call `apply_committed()` after this to apply newly committed entries.
	///
	/// Called when:
	/// - Leader determines an entry is replicated on a majority
	/// - Follower receives `leader_commit` in `AppendEntries`
	pub fn commit(&mut self, new_commit_index: Index) {
		if new_commit_index <= self.committed() {
			return;
		}

		// Don't commit beyond what we have
		self.commit_index.send_if_modified(|last_index| {
			let prev_value = *last_index;
			let new_value = new_commit_index.min(self.index());
			*last_index = new_value;
			prev_value != new_value
		});
	}

	/// Applies all committed entries that haven't been applied yet.
	///
	/// Calls `apply_fn` for each entry in order.
	/// The function receives the command and the log index.
	pub fn apply_committed(&mut self, mut apply_fn: impl FnMut(&C, Index)) {
		while self.applied() < self.committed() {
			self
				.last_applied
				.send_modify(|last_applied| *last_applied += 1);

			if let Some(entry) = self.get(self.applied()) {
				apply_fn(&entry.command, entry.index);
			}
		}
	}

	/// Returns entries starting from `start_index` for replication.
	/// Used by leader to send entries to followers.
	pub fn entries_from(
		&self,
		start_index: Index,
	) -> impl Iterator<Item = &LogEntry<C>> {
		let start = start_index as usize;
		self.entries.iter().skip(start)
	}

	/// Checks if our log is at least as up-to-date as the given term/index.
	/// Used for vote decisions in RequestVote message.
	///
	/// Raft paper §5.4.1: Compare by term first, then by index.
	pub fn is_up_to_date(
		&self,
		candidate_last_term: Term,
		candidate_last_index: Index,
	) -> bool {
		let my_last_term = self.term();
		let my_last_index = self.index();

		match candidate_last_term.cmp(&my_last_term) {
			std::cmp::Ordering::Greater => true,
			std::cmp::Ordering::Less => false,
			std::cmp::Ordering::Equal => candidate_last_index >= my_last_index,
		}
	}

	/// Returns true if there are committed entries that haven't been applied.
	pub fn has_unapplied(&self) -> bool {
		self.applied() < self.committed()
	}
}
