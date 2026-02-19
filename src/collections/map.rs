use {
	super::{
		Error,
		READER,
		WRITER,
		When,
		primitives::{Key, StoreId, Value, Version},
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

pub type MapWriter<K, V> = Map<K, V, WRITER>;
pub type MapReader<K, V> = Map<K, V, READER>;

/// Replicated, unordered, eventually consistent map.
pub struct Map<K: Key, V: Value, const IS_WRITER: bool = WRITER> {
	when: When,
	group: Group<MapStateMachine<K, V>>,
	snapshot: watch::Receiver<im::HashMap<K, V>>,
}

// read-only access, available to both writers and readers
impl<K: Key, V: Value, const IS_WRITER: bool> Map<K, V, IS_WRITER> {
	/// Get the number of entries in the map.
	///
	/// Time: O(1)
	pub fn len(&self) -> usize {
		self.snapshot.borrow().len()
	}

	/// Test whether the map is empty.
	///
	/// Time: O(1)
	pub fn is_empty(&self) -> bool {
		self.snapshot.borrow().is_empty()
	}

	/// Test whether the map contains a given key.
	///
	/// Time: O(log n)
	pub fn contains_key(&self, key: &K) -> bool {
		self.snapshot.borrow().contains_key(key)
	}

	/// Get the value for a given key, if it exists.
	///
	/// Time: O(log n)
	pub fn get(&self, key: &K) -> Option<V> {
		self.snapshot.borrow().get(key).cloned()
	}

	/// Get an iterator over the key-value pairs in the map.
	pub fn iter(&self) -> impl Iterator<Item = (K, V)> {
		let iter_clone = self.snapshot.borrow().clone();
		iter_clone.into_iter()
	}

	/// Get an iterator over the keys in the map.
	pub fn keys(&self) -> impl Iterator<Item = K> {
		let iter_clone = self.snapshot.borrow().clone();
		iter_clone.into_iter().map(|(k, _)| k)
	}

	/// Get an iterator over the values in the map.
	pub fn values(&self) -> impl Iterator<Item = V> {
		let iter_clone = self.snapshot.borrow().clone();
		iter_clone.into_iter().map(|(_, v)| v)
	}

	/// Returns an observer of the map's state, which can be used to wait for
	/// the map to reach a certain state version before performing an action
	/// or knowing when it is online or offline.
	pub const fn when(&self) -> &When {
		&self.when
	}

	/// The current version of the vector's state, which is the version of the
	/// latest committed state.
	pub fn version(&self) -> Version {
		Version(self.group.committed())
	}
}

// Mutable operations, only available to writers
impl<K: Key, V: Value> MapWriter<K, V> {
	/// Discard all entries from the map.
	///
	/// This leaves you with an empty map, and all entries that
	/// were previously inside it are dropped.
	pub async fn clear(&self) -> Result<Version, Error<()>> {
		self
			.execute(MapCommand::Clear, |_| Error::Offline(()))
			.await
	}

	/// Insert a key-value pair into the map.
	///
	/// If the map already contained this key, the value is updated.
	///
	/// Time: O(log n)
	pub async fn insert(
		&self,
		key: K,
		value: V,
	) -> Result<Version, Error<(K, V)>> {
		self
			.execute(MapCommand::Insert { key, value }, |cmd| match cmd {
				MapCommand::Insert { key, value } => Error::Offline((key, value)),
				_ => unreachable!(),
			})
			.await
	}

	/// Insert multiple key-value pairs into the map.
	pub async fn extend(
		&self,
		entries: impl IntoIterator<Item = (K, V)>,
	) -> Result<Version, Error<Vec<(K, V)>>> {
		let entries: Vec<(K, V)> = entries.into_iter().collect();

		if entries.is_empty() {
			return Ok(Version(self.group.committed()));
		}

		self
			.execute(MapCommand::Extend { entries }, |cmd| match cmd {
				MapCommand::Extend { entries } => Error::Offline(entries),
				_ => unreachable!(),
			})
			.await
	}

	/// Remove a key-value pair from the map.
	///
	/// Time: O(log n)
	pub async fn remove(&self, key: &K) -> Result<Version, Error<K>> {
		let key = key.clone();
		self
			.execute(MapCommand::Remove { key }, |cmd| match cmd {
				MapCommand::Remove { key } => Error::Offline(key),
				_ => unreachable!(),
			})
			.await
	}
}

// construction
impl<K: Key, V: Value, const IS_WRITER: bool> Map<K, V, IS_WRITER> {
	/// Create a new map in writer mode.
	///
	/// The returned writer can be used to modify the map, and it also provides
	/// read access to the map's contents. Writers can be used by multiple
	/// nodes concurrently, and all changes made by any writer will be replicated
	/// to all other writers and readers.
	pub fn writer(network: &Network, store_id: StoreId) -> MapWriter<K, V> {
		let (machine, snapshot) = MapStateMachine::new(store_id, true);
		let group = network
			.groups()
			.with_key(store_id.into())
			.with_state_machine(machine)
			.join();

		MapWriter::<K, V> {
			when: When::new(group.when().clone()),
			group,
			snapshot,
		}
	}

	/// Create a new map in writer mode.
	pub fn new(network: &Network, store_id: StoreId) -> MapWriter<K, V> {
		MapWriter::<K, V>::writer(network, store_id)
	}

	/// Create a new map in reader mode.
	///
	/// The returned reader provides read-only access to the map's contents.
	/// Readers can be used by multiple nodes concurrently, and they will see all
	/// changes made by any writer. However, readers cannot modify the map, and
	/// they will not be able to make any changes themselves. Readers have longer
	/// election timeouts to reduce the likelihood of them being elected as
	/// group leaders, which reduces latency for read operations.
	pub fn reader(network: &Network, store_id: StoreId) -> MapReader<K, V> {
		let (machine, snapshot) = MapStateMachine::new(store_id, false);
		let group = network
			.groups()
			.with_key(store_id.into())
			.with_state_machine(machine)
			.join();

		MapReader::<K, V> {
			when: When::new(group.when().clone()),
			group,
			snapshot,
		}
	}
}

// internal
impl<K: Key, V: Value> Map<K, V, WRITER> {
	async fn execute<TErr>(
		&self,
		command: MapCommand<K, V>,
		offline_err: impl FnOnce(MapCommand<K, V>) -> Error<TErr>,
	) -> Result<Version, Error<TErr>> {
		self
			.group
			.execute(command)
			.await
			.map(Version)
			.map_err(|e| match e {
				CommandError::Offline(mut items) => {
					let command = items.remove(0);
					offline_err(command)
				}
				CommandError::GroupTerminated => Error::NetworkDown,
				CommandError::NoCommands => unreachable!(),
			})
	}
}

struct MapStateMachine<K: Key, V: Value> {
	data: im::HashMap<K, V>,
	latest: watch::Sender<im::HashMap<K, V>>,
	store_id: StoreId,
	is_writer: bool,
}

impl<K: Key, V: Value> MapStateMachine<K, V> {
	pub fn new(
		store_id: StoreId,
		is_writer: bool,
	) -> (Self, watch::Receiver<im::HashMap<K, V>>) {
		let data = im::HashMap::new();
		let (latest, snapshot) = watch::channel(data.clone());
		(
			Self {
				data,
				latest,
				store_id,
				is_writer,
			},
			snapshot,
		)
	}
}

impl<K: Key, V: Value> StateMachine for MapStateMachine<K, V> {
	type Command = MapCommand<K, V>;
	type Query = ();
	type QueryResult = ();
	type StateSync = LogReplaySync<Self>;

	fn apply(&mut self, command: Self::Command) {
		self.apply_batch([command]);
	}

	fn apply_batch(&mut self, commands: impl IntoIterator<Item = Self::Command>) {
		for command in commands {
			match command {
				MapCommand::Clear => {
					self.data.clear();
				}
				MapCommand::Insert { key, value } => {
					self.data.insert(key, value);
				}
				MapCommand::Remove { key } => {
					self.data.remove(&key);
				}
				MapCommand::Extend { entries } => {
					for (key, value) in entries {
						self.data.insert(key, value);
					}
				}
			}
		}
		self.latest.send_replace(self.data.clone());
	}

	/// The group-key for a map is derived from the store ID and the types of
	/// the map's keys and values. This ensures that different maps (with
	/// different store IDs or key/value types) will be in different groups.
	fn signature(&self) -> crate::UniqueId {
		UniqueId::from("mosaik_collections_map")
			.derive(self.store_id)
			.derive(type_name::<K>())
			.derive(type_name::<V>())
	}

	/// This state machine doesn't support external queries, because all of its
	/// state is observable through the `latest` watch channel. Therefore, the
	/// query method is a no-op.
	fn query(&self, (): Self::Query) {}

	fn state_sync(&self) -> Self::StateSync {
		LogReplaySync::default()
	}

	fn consensus_config(&self) -> Option<ConsensusConfig> {
		let mut config = ConsensusConfig::default();

		if !self.is_writer {
			// Readers have longer election timeouts to reduce the likelihood of them
			// being elected as leaders. This is an optimization to reduce latency.
			config.election_timeout *= 3;
			config.bootstrap_delay *= 3;
		}

		Some(config)
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "K: Key, V: Value")]
enum MapCommand<K, V> {
	Clear,
	Insert { key: K, value: V },
	Remove { key: K },
	Extend { entries: Vec<(K, V)> },
}
