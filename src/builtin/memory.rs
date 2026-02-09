#![allow(clippy::cast_possible_truncation)]

use crate::groups::{Command, Index, Storage, Term};

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
		self.entries.len() as Index
	}

	fn get(&self, index: Index) -> Option<(C, Term)> {
		if index == 0 {
			return None;
		}

		self.entries.get((index - 1) as usize).cloned()
	}

	fn get_range(
		&self,
		range: std::ops::Range<Index>,
	) -> impl Iterator<Item = (Term, Index, C)> + '_ {
		let start = (range.start - 1) as usize;
		let end = (range.end - 1) as usize;
		self
			.entries
			.iter()
			.enumerate()
			.skip(start)
			.take(end - start)
			.map(move |(i, (cmd, term))| {
				(*term, (start + i + 1) as Index, cmd.clone())
			})
	}

	fn truncate(&mut self, at: Index) {
		if at <= 1 {
			self.entries.clear();
		} else {
			self.entries.truncate((at - 1) as usize);
		}
	}

	fn last(&self) -> Option<(Term, Index)> {
		if self.entries.is_empty() {
			None
		} else {
			let (_, term) = self.entries.last().unwrap();
			Some((*term, self.entries.len() as Index))
		}
	}
}
