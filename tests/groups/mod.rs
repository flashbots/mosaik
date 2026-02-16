use {
	core::num::NonZero,
	futures::{StreamExt, stream::FuturesUnordered},
	mosaik::{
		PeerId,
		groups::{LogReplaySync, StateMachine},
		primitives::UniqueId,
	},
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
	sync_batch_size: NonZero<u64>,
}

impl Default for Counter {
	fn default() -> Self {
		Self {
			value: 0,
			sync_batch_size: NonZero::new(1000).unwrap(),
		}
	}
}

impl Counter {
	pub const fn with_sync_batch_size(mut self, sync_batch_size: u64) -> Self {
		self.sync_batch_size = NonZero::new(sync_batch_size).unwrap();
		self
	}
}

impl StateMachine for Counter {
	type Command = CounterCommand;
	type Query = CounterValueQuery;
	type QueryResult = i64;
	type StateSync = LogReplaySync<Self>;

	fn signature(&self) -> UniqueId {
		UniqueId::from_u8(2)
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

	fn sync_factory(&self) -> Self::StateSync {
		LogReplaySync::default().with_batch_size(self.sync_batch_size)
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

		if let Some(first_leader) = leaders.first().and_then(|l| *l)
			&& leaders.iter().all(|&l| l == Some(first_leader))
		{
			return first_leader;
		}

		let mut changes: FuturesUnordered<_> =
			groups.iter().map(|g| g.when().leader_changed()).collect();
		changes.next().await.unwrap();
	}
}
