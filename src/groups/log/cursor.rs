use {
	core::{cmp::Ordering, fmt},
	derive_more::{From, Into},
	serde::{Deserialize, Serialize},
};

/// Raft term, increases monotonically with every new leader election.
pub type Term = u64;

/// Raft log index increases monotonically with every new log entry.
pub type Index = u64;

/// Progress of the log.
#[derive(
	Debug,
	Default,
	Clone,
	Copy,
	PartialEq,
	Eq,
	Hash,
	Serialize,
	Deserialize,
	From,
	Into,
)]
pub struct Cursor(pub Term, pub Index);

impl Cursor {
	pub const fn new(term: Term, index: Index) -> Self {
		Self(term, index)
	}

	pub const fn term(&self) -> Term {
		self.0
	}

	pub const fn index(&self) -> Index {
		self.1
	}

	pub const fn is_behind(&self, other: &Self) -> bool {
		(self.term() < other.term()) || (self.index() < other.index())
	}
}

impl PartialOrd for Cursor {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		Some(self.cmp(other))
	}
}

impl Ord for Cursor {
	fn cmp(&self, other: &Self) -> Ordering {
		self
			.term()
			.cmp(&other.term())
			.then(self.index().cmp(&other.index()))
	}
}

impl fmt::Display for Cursor {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "(t:{},i:{})", self.term(), self.index())
	}
}
