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

/// Replicated, unordered, eventually consistent map.
pub struct Map<K: Key, V: Value> {
	when: When,
	group: Group<MapStateMachine<K, V>>,
	snapshot: watch::Receiver<im::HashMap<K, V>>,
}

impl<K: Key, V: Value> Map<K, V> {
	pub fn writer(network: &Network, store_id: StoreId) -> Self {
		let (state_machine, snapshot) = MapStateMachine::new(store_id, false);
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

	pub fn reader(network: &Network, store_id: StoreId) -> MapReader<K, V> {
		MapReader::new(network, store_id)
	}

	pub async fn insert(
		&self,
		key: K,
		value: V,
	) -> Result<Version, InsertError<(K, V)>> {
		self
			.group
			.execute(MapCommand::Insert { key, value })
			.await
			.map(Version)
			.map_err(|e| {
				InsertError::Offline(match e {
					CommandError::Offline(mut items) => {
						let MapCommand::Insert { key, value } = items.remove(0) else {
							unreachable!()
						};
						(key, value)
					}
					CommandError::NoCommands => unreachable!(),
					CommandError::GroupTerminated => {
						unreachable!("group lifetime is tied to the map instance")
					}
				})
			})
	}

	pub async fn extend(
		&self,
		entries: impl IntoIterator<Item = (K, V)>,
	) -> Result<Version, InsertManyError<(K, V)>> {
		self
			.group
			.execute_many(
				entries
					.into_iter()
					.map(|(key, value)| MapCommand::Insert { key, value }),
			)
			.await
			.map(|range| Version(*range.end()))
			.map_err(|e| {
				InsertManyError::Offline(match e {
					CommandError::Offline(items) => items
						.into_iter()
						.map(|item| {
							let MapCommand::Insert { key, value } = item else {
								unreachable!()
							};
							(key, value)
						})
						.collect(),
					CommandError::NoCommands => unreachable!(),
					CommandError::GroupTerminated => {
						unreachable!("group lifetime is tied to the map instance")
					}
				})
			})
	}

	pub async fn remove(&self, key: K) -> Result<Version, RemoveError<K>> {
		self
			.group
			.execute(MapCommand::Remove { key: key.clone() })
			.await
			.map(Version)
			.map_err(|e| {
				RemoveError::Offline(match e {
					CommandError::Offline(mut items) => {
						let MapCommand::Remove { key } = items.remove(0) else {
							unreachable!()
						};
						key
					}
					CommandError::NoCommands => unreachable!(),
					CommandError::GroupTerminated => {
						unreachable!("group lifetime is tied to the map instance")
					}
				})
			})
	}

	pub fn is_empty(&self) -> bool {
		self.snapshot.borrow().is_empty()
	}

	pub fn len(&self) -> usize {
		self.snapshot.borrow().len()
	}

	pub fn get(&self, key: &K) -> Option<V> {
		self.snapshot.borrow().get(key).cloned()
	}

	pub fn contains_key(&self, key: &K) -> bool {
		self.snapshot.borrow().contains_key(key)
	}

	pub fn version(&self) -> Version {
		Version(self.group.committed())
	}

	pub const fn when(&self) -> &When {
		&self.when
	}
}

pub struct MapReader<K: Key, V: Value> {
	when: When,
	group: Group<MapStateMachine<K, V>>,
	snapshot: watch::Receiver<im::HashMap<K, V>>,
}

impl<K: Key, V: Value> MapReader<K, V> {
	pub fn new(network: &Network, store_id: StoreId) -> Self {
		let (state_machine, snapshot) = MapStateMachine::new(store_id, true);
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

impl<K: Key, V: Value> MapReader<K, V> {
	pub fn is_empty(&self) -> bool {
		self.snapshot.borrow().is_empty()
	}

	pub fn len(&self) -> usize {
		self.snapshot.borrow().len()
	}

	pub fn get(&self, key: &K) -> Option<V> {
		self.snapshot.borrow().get(key).cloned()
	}

	pub fn contains_key(&self, key: &K) -> bool {
		self.snapshot.borrow().contains_key(key)
	}

	pub fn version(&self) -> Version {
		Version(self.group.committed())
	}

	pub const fn when(&self) -> &When {
		&self.when
	}
}

struct MapStateMachine<K: Key, V: Value> {
	map: im::HashMap<K, V>,
	latest: watch::Sender<im::HashMap<K, V>>,
	store_id: StoreId,
	reader: bool,
}

impl<K: Key, V: Value> MapStateMachine<K, V> {
	fn new(
		store_id: StoreId,
		reader: bool,
	) -> (Self, watch::Receiver<im::HashMap<K, V>>) {
		let (latest, receiver) = watch::channel(im::HashMap::new());
		(
			Self {
				store_id,
				map: im::HashMap::new(),
				latest,
				reader,
			},
			receiver,
		)
	}
}

impl<K: Key, V: Value> StateMachine for MapStateMachine<K, V> {
	type Command = MapCommand<K, V>;
	type Query = MapQuery<K>;
	type QueryResult = MapQueryResult<V>;
	type StateSync = LogReplaySync<Self>;

	fn signature(&self) -> UniqueId {
		UniqueId::from("mosaik_collections_map_state_machine")
			.derive(self.store_id)
			.derive(type_name::<K>())
			.derive(type_name::<V>())
	}

	fn apply(&mut self, command: Self::Command) {
		match command {
			MapCommand::Insert { key, value } => {
				self.map.insert(key, value);
				self.latest.send_replace(self.map.clone());
			}
			MapCommand::Remove { key } => {
				self.map.remove(&key);
				self.latest.send_replace(self.map.clone());
			}
		}
	}

	fn query(&self, query: Self::Query) -> Self::QueryResult {
		match query {
			MapQuery::GetLen => MapQueryResult::Len(self.map.len()),
			MapQuery::ContainsKey(key) => {
				MapQueryResult::ContainsKey(self.map.contains_key(&key))
			}
			MapQuery::Get(key) => MapQueryResult::Get(self.map.get(&key).cloned()),
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

#[derive(Clone, Serialize, Deserialize)]
#[serde(bound = "K: Key, V: Value")]
enum MapCommand<K, V> {
	Insert { key: K, value: V },
	Remove { key: K },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(bound = "K: Key")]
enum MapQuery<K> {
	GetLen,
	ContainsKey(K),
	Get(K),
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(bound = "V: Value")]
enum MapQueryResult<V> {
	Len(usize),
	ContainsKey(bool),
	Get(Option<V>),
}

pub trait Key:
	Clone
	+ serde::Serialize
	+ serde::de::DeserializeOwned
	+ core::hash::Hash
	+ PartialEq
	+ Eq
	+ Send
	+ Sync
	+ Unpin
	+ 'static
{
}

impl<T> Key for T where
	T: Clone
		+ serde::Serialize
		+ serde::de::DeserializeOwned
		+ core::hash::Hash
		+ PartialEq
		+ Eq
		+ Send
		+ Sync
		+ Unpin
		+ 'static
{
}
