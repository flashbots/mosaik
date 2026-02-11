use {
	mosaik::{groups::StateMachine, primitives::UniqueId, unique_id},
	serde::{Deserialize, Serialize},
};

mod bonds;
mod builder;
mod leader;
mod rsm;

#[derive(Debug, Default)]
struct Counter {
	value: i64,
}

impl StateMachine for Counter {
	type Command = CounterCommand;
	type Query = CounterValueQuery;
	type QueryResult = i64;

	const ID: UniqueId = unique_id!(
		"0000000000000000000000000000000000000000000000000000000000000002"
	);

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum CounterCommand {
	Increment(u32),
	Decrement(u32),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CounterValueQuery;
