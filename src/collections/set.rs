use {
	super::{StoreId, When},
	crate::{
		Group,
		Network,
		UniqueId,
		groups::{CommandError, ConsensusConfig, LogReplaySync, StateMachine},
	},
	core::any::type_name,
	serde::{Deserialize, Serialize},
	tokio::sync::watch,
};

/// Replicated, unordered, eventually consistent set.
pub struct Set<T: Value> {
	when: When,
	group: Group<SetStateMachine<T>>,
	snapshot: watch::Receiver<im::HashSet<T>>,
}

impl<T: Value> Set<T> {
	pub fn writer(network: &Network, store_id: StoreId) -> Self {
		let (state_machine, snapshot) = SetStateMachine::new(store_id, false);
		let group = network
			.groups()
			.with_key(store_id.into())
			.with_state_machine(state_machine)
			.join();

		Self {
			when: When::new(group.when().clone()),
			group,
			snapshot,
		}
	}

	pub fn reader(network: &Network, store_id: StoreId) -> SetReader<T> {
		SetReader::new(network, store_id)
	}

	pub async fn insert(&self, value: T) -> Result<Version, InsertError<T>> {
		self
			.group
			.execute(SetCommand::Insert { value })
			.await
			.map(Version)
			.map_err(|e| match e {
				CommandError::Offline(mut items) => {
					let SetCommand::Insert { value } = items.remove(0) else {
						unreachable!()
					};
					InsertError::Offline(value)
				}
				CommandError::NoCommands => unreachable!(),
				CommandError::GroupTerminated => InsertError::NetworkDown,
			})
	}

	pub async fn extend(
		&self,
		entries: impl IntoIterator<Item = T>,
	) -> Result<Version, InsertManyError<T>> {
		self
			.group
			.execute_many(
				entries
					.into_iter()
					.map(|value| SetCommand::Insert { value }),
			)
			.await
			.map(|range| Version(*range.end()))
			.map_err(|e| match e {
				CommandError::Offline(items) => InsertManyError::Offline(
					items
						.into_iter()
						.map(|item| {
							let SetCommand::Insert { value } = item else {
								unreachable!()
							};
							value
						})
						.collect(),
				),
				CommandError::NoCommands => unreachable!(),
				CommandError::GroupTerminated => InsertManyError::NetworkDown,
			})
	}

	pub async fn remove(&self, value: T) -> Result<Version, RemoveError<T>> {
		self
			.group
			.execute(SetCommand::Remove { value })
			.await
			.map(Version)
			.map_err(|e| match e {
				CommandError::Offline(mut items) => {
					let SetCommand::Remove { value } = items.remove(0) else {
						unreachable!()
					};
					RemoveError::Offline(value)
				}
				CommandError::NoCommands => unreachable!(),
				CommandError::GroupTerminated => RemoveError::NetworkDown,
			})
	}

	pub fn is_empty(&self) -> bool {
		self.snapshot.borrow().is_empty()
	}

	pub fn len(&self) -> usize {
		self.snapshot.borrow().len()
	}

	pub fn contains(&self, value: &T) -> bool {
		self.snapshot.borrow().contains(value)
	}

	pub fn version(&self) -> Version {
		Version(self.group.committed())
	}

	pub const fn when(&self) -> &When {
		&self.when
	}
}

pub struct SetReader<T: Value> {
	when: When,
	group: Group<SetStateMachine<T>>,
	snapshot: watch::Receiver<im::HashSet<T>>,
}

impl<T: Value> SetReader<T> {
	pub fn new(network: &Network, store_id: StoreId) -> Self {
		let (state_machine, snapshot) = SetStateMachine::new(store_id, true);
		let group = network
			.groups()
			.with_key(store_id.into())
			.with_state_machine(state_machine)
			.join();

		Self {
			when: When::new(group.when().clone()),
			group,
			snapshot,
		}
	}
}

impl<T: Value> SetReader<T> {
	pub fn is_empty(&self) -> bool {
		self.snapshot.borrow().is_empty()
	}

	pub fn len(&self) -> usize {
		self.snapshot.borrow().len()
	}

	pub fn contains(&self, value: &T) -> bool {
		self.snapshot.borrow().contains(value)
	}

	pub fn version(&self) -> Version {
		Version(self.group.committed())
	}

	pub const fn when(&self) -> &When {
		&self.when
	}
}

struct SetStateMachine<T: Value> {
	data: im::HashSet<T>,
	latest: watch::Sender<im::HashSet<T>>,
	store_id: StoreId,
	reader: bool,
}

impl<T: Value> SetStateMachine<T> {
	fn new(
		store_id: StoreId,
		reader: bool,
	) -> (Self, watch::Receiver<im::HashSet<T>>) {
		let (latest, snapshot) = watch::channel(im::HashSet::new());
		(
			Self {
				data: im::HashSet::new(),
				latest,
				store_id,
				reader,
			},
			snapshot,
		)
	}
}

impl<T: Value> StateMachine for SetStateMachine<T> {
	type Command = SetCommand<T>;
	type Query = SetQuery<T>;
	type QueryResult = SetQueryResult;
	type StateSync = LogReplaySync<Self>;

	fn signature(&self) -> UniqueId {
		UniqueId::from("mosaik_collections_set_state_machine")
			.derive(self.store_id)
			.derive(type_name::<T>())
	}

	fn apply(&mut self, command: Self::Command) {
		match command {
			SetCommand::Insert { value } => {
				self.data.insert(value);
			}
			SetCommand::Remove { value } => {
				self.data.remove(&value);
			}
		}
		self.latest.send_replace(self.data.clone());
	}

	fn apply_batch(&mut self, commands: impl IntoIterator<Item = Self::Command>) {
		for command in commands {
			match command {
				SetCommand::Insert { value } => {
					self.data.insert(value);
				}
				SetCommand::Remove { value } => {
					self.data.remove(&value);
				}
			}
		}
		self.latest.send_replace(self.data.clone());
	}

	fn query(&self, query: Self::Query) -> Self::QueryResult {
		match query {
			SetQuery::GetLen => SetQueryResult::Len(self.data.len()),
			SetQuery::Contains { value } => {
				SetQueryResult::Contains(self.data.contains(&value))
			}
		}
	}

	fn state_sync(&self) -> Self::StateSync {
		LogReplaySync::default()
	}

	fn consensus_config(&self) -> Option<ConsensusConfig> {
		let mut config = ConsensusConfig::default();

		if self.reader {
			config.election_timeout *= 3;
			config.bootstrap_delay *= 3;
		}

		Some(config)
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "T: Value")]
enum SetCommand<T> {
	Insert { value: T },
	Remove { value: T },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "T: Value")]
enum SetQuery<T> {
	GetLen,
	Contains { value: T },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum SetQueryResult {
	Len(usize),
	Contains(bool),
}
