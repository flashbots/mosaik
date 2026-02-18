use {
	super::{
		InsertError,
		InsertManyError,
		RemoveError,
		StoreId,
		Value,
		Version,
		When,
	},
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

pub struct Vec<T: Value> {
	when: When,
	group: Group<VecStateMachine<T>>,
	snapshot: watch::Receiver<im::Vector<T>>,
}

impl<T: Value> Vec<T> {
	pub fn writer(
		network: &crate::network::Network,
		store_id: crate::UniqueId,
	) -> Self {
		let (machine, snapshot) = VecStateMachine::new(store_id, false);
		let group = network
			.groups()
			.with_key(store_id.into())
			.with_state_machine(machine)
			.join();

		Self {
			when: When::new(group.when().clone()),
			group,
			snapshot,
		}
	}

	pub fn reader(network: &Network, store_id: StoreId) -> VecReader<T> {
		VecReader::new(network, store_id)
	}

	pub async fn push(&self, value: T) -> Result<Version, InsertError<T>> {
		self
			.group
			.execute(VecCommand::Push { value })
			.await
			.map(Version)
			.map_err(|e| match e {
				CommandError::Offline(mut items) => {
					let VecCommand::Push { value } = items.remove(0) else {
						unreachable!();
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
		let entries: std::vec::Vec<T> = entries.into_iter().collect();

		if entries.is_empty() {
			return Ok(Version(self.group.committed()));
		}

		self
			.group
			.execute(VecCommand::Extend { entries })
			.await
			.map(Version)
			.map_err(|e| match e {
				CommandError::Offline(mut commands) => InsertManyError::Offline({
					let VecCommand::Extend { entries } = commands.remove(0) else {
						unreachable!()
					};
					entries
				}),
				CommandError::NoCommands => unreachable!(),
				CommandError::GroupTerminated => InsertManyError::NetworkDown,
			})
	}

	pub async fn remove(&self, index: u64) -> Result<Version, RemoveError<u64>> {
		self
			.group
			.execute(VecCommand::Remove { index })
			.await
			.map(Version)
			.map_err(|e| match e {
				CommandError::Offline(mut items) => {
					let VecCommand::Remove { index } = items.remove(0) else {
						unreachable!();
					};
					RemoveError::Offline(index)
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

	pub fn get(&self, index: u64) -> Option<T> {
		self.snapshot.borrow().get(index as usize).cloned()
	}

	pub const fn when(&self) -> &When {
		&self.when
	}
}

pub struct VecReader<T: Value> {
	when: When,
	group: Group<VecStateMachine<T>>,
	snapshot: watch::Receiver<im::Vector<T>>,
}

impl<T: Value> VecReader<T> {
	pub fn new(network: &Network, store_id: StoreId) -> Self {
		let (machine, snapshot) = VecStateMachine::new(store_id, true);
		let group = network
			.groups()
			.with_key(store_id.into())
			.with_state_machine(machine)
			.join();

		Self {
			when: When::new(group.when().clone()),
			group,
			snapshot,
		}
	}
}

impl<T: Value> VecReader<T> {
	pub fn is_empty(&self) -> bool {
		self.snapshot.borrow().is_empty()
	}

	pub fn len(&self) -> usize {
		self.snapshot.borrow().len()
	}

	pub fn get(&self, index: u64) -> Option<T> {
		self.snapshot.borrow().get(index as usize).cloned()
	}

	pub fn version(&self) -> Version {
		Version(self.group.committed())
	}

	pub const fn when(&self) -> &When {
		&self.when
	}
}

struct VecStateMachine<T: Value> {
	data: im::Vector<T>,
	latest: watch::Sender<im::Vector<T>>,
	store_id: StoreId,
	reader: bool,
}

impl<T: Value> VecStateMachine<T> {
	pub fn new(
		store_id: StoreId,
		reader: bool,
	) -> (Self, watch::Receiver<im::Vector<T>>) {
		let data = im::Vector::new();
		let (latest, snapshot) = watch::channel(data.clone());
		(
			Self {
				data,
				latest,
				store_id,
				reader,
			},
			snapshot,
		)
	}
}

impl<T: Value> StateMachine for VecStateMachine<T> {
	type Command = VecCommand<T>;
	type Query = VecQuery;
	type QueryResult = VecQueryResult<T>;
	type StateSync = LogReplaySync<Self>;

	fn apply(&mut self, command: Self::Command) {
		match command {
			VecCommand::Push { value } => {
				self.data.push_back(value);
			}
			VecCommand::Extend { entries } => {
				self.data.extend(entries);
			}
			VecCommand::Remove { index } => {
				self.data.remove(index as usize);
			}
		}

		self.latest.send_replace(self.data.clone());
	}

	fn apply_batch(&mut self, commands: impl IntoIterator<Item = Self::Command>) {
		for command in commands {
			match command {
				VecCommand::Push { value } => {
					self.data.push_back(value);
				}
				VecCommand::Extend { entries } => {
					self.data.extend(entries);
				}
				VecCommand::Remove { index } => {
					self.data.remove(index as usize);
				}
			}
		}
		self.latest.send_replace(self.data.clone());
	}

	fn signature(&self) -> crate::UniqueId {
		UniqueId::from("mosaik_collections_vec_state_machine")
			.derive(self.store_id)
			.derive(type_name::<T>())
	}

	fn query(&self, query: Self::Query) -> Self::QueryResult {
		match query {
			VecQuery::GetLen => VecQueryResult::Len(self.data.len()),
			VecQuery::Get { index } => {
				let value = self.data.get(index as usize).cloned();
				VecQueryResult::Get(value)
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
enum VecCommand<T> {
	Push { value: T },
	Extend { entries: std::vec::Vec<T> },
	Remove { index: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum VecQuery {
	GetLen,
	Get { index: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum VecQueryResult<T> {
	Len(usize),
	Get(Option<T>),
}
