use {
	core::num::NonZero,
	futures::{StreamExt, stream::FuturesUnordered},
	mosaik::{PeerId, groups::StateMachine, primitives::UniqueId, unique_id},
	serde::{Deserialize, Serialize},
};

mod bonds;
mod builder;
mod catchup;
mod execute;
mod feed;
mod leader;

#[derive(Debug)]
struct Counter {
	value: i64,
	chunk_size: NonZero<u64>,
}

impl Default for Counter {
	fn default() -> Self {
		Self {
			value: 0,
			chunk_size: NonZero::new(1000).unwrap(),
		}
	}
}

impl Counter {
	pub const fn with_chunk_size(mut self, chunk_size: u64) -> Self {
		self.chunk_size = NonZero::new(chunk_size).unwrap();
		self
	}
}

impl StateMachine for Counter {
	type Command = CounterCommand;
	type Query = CounterValueQuery;
	type QueryResult = i64;

	const ID: UniqueId = unique_id!(
		"0000000000000000000000000000000000000000000000000000000000000002"
	);

	fn reset(&mut self) {
		self.value = 0;
	}

	fn apply(&mut self, command: Self::Command) {
		match command {
			CounterCommand::Increment(n) => {
				self.value = self.value.wrapping_add(i64::from(n));
			}
			CounterCommand::Decrement(n) => {
				self.value = self.value.wrapping_sub(i64::from(n));
			}
		}
	}

	fn query(&self, query: Self::Query) -> Self::QueryResult {
		match query {
			CounterValueQuery => self.value,
		}
	}

	fn catchup_chunk_size(&self) -> NonZero<u64> {
		self.chunk_size
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum CounterCommand {
	Increment(u32),
	Decrement(u32),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CounterValueQuery;

/// Resolves when all groups have converged on the same leader and returns the
/// ID of the leader.
async fn leaders_converged<M: StateMachine>(
	groups: impl IntoIterator<Item = &mosaik::Group<M>>,
) -> PeerId {
	let groups = groups.into_iter().collect::<Vec<_>>();
	assert!(!groups.is_empty());

	loop {
		let leaders = groups.iter().map(|g| g.leader()).collect::<Vec<_>>();

		if let Some(first_leader) = leaders.first().and_then(|l| *l) {
			if leaders.iter().all(|&l| l == Some(first_leader)) {
				return first_leader;
			}
		}

		let mut changes: FuturesUnordered<_> =
			groups.iter().map(|g| g.when().leader_changed()).collect();
		changes.next().await.unwrap();
	}
}
