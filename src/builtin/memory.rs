#![allow(clippy::cast_possible_truncation)]

use {
	crate::groups::{Command, Cursor, Index, Storage, Term},
	core::ops::RangeInclusive,
};

/// An in-memory implementation of the `Storage` trait replicated log storage in
/// groups. This storage implementation is compatible with arbitrary command
/// types.
#[derive(Debug)]
pub struct InMemory<C: Command> {
	entries: Vec<(C, Term)>,
}

impl<C: Command> Default for InMemory<C> {
	fn default() -> Self {
		Self {
			entries: Vec::new(),
		}
	}
}

impl<C: Command> Storage<C> for InMemory<C> {
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

	fn get_range(
		&self,
		range: RangeInclusive<Index>,
	) -> impl Iterator<Item = (Term, Index, C)> + '_ {
		let start: usize = range.start().prev().into();
		let end: usize = range.end().prev().into();

		self
			.entries
			.iter()
			.enumerate()
			.skip(start)
			.take(end - start + 1)
			.map(move |(i, (cmd, term))| (*term, (start + i + 1).into(), cmd.clone()))
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
