use {
	super::{
		Error,
		READER,
		SyncConfig,
		WRITER,
		When,
		primitives::{Key, StoreId, Value, Version},
	},
	crate::{
		Group,
		Network,
		PeerId,
		UniqueId,
		collections::sync::{
			Snapshot,
			SnapshotStateMachine,
			SnapshotSync,
			protocol::SnapshotRequest,
		},
		groups::{
			ApplyContext,
			CommandError,
			ConsensusConfig,
			Cursor,
			LogReplaySync,
			StateMachine,
		},
	},
	core::any::type_name,
	serde::{Deserialize, Serialize},
	std::hash::BuildHasherDefault,
	tokio::sync::watch,
};

/// Deterministic hasher for the internal `im::HashMap`, ensuring that
/// iteration order is identical across all nodes for the same map state.
/// Uses `DefaultHasher` (SipHash-1-3) with a fixed zero seed.
type HashMap<K, V> =
	im::HashMap<K, V, BuildHasherDefault<std::hash::DefaultHasher>>;

pub type MapWriter<K, V> = Map<K, V, WRITER>;
pub type MapReader<K, V> = Map<K, V, READER>;

/// Replicated, unordered, eventually consistent map.
pub struct Map<K: Key, V: Value, const IS_WRITER: bool = WRITER> {
	when: When,
	group: Group<MapStateMachine<K, V>>,
	data: watch::Receiver<HashMap<K, V>>,
}

// read-only access, available to both writers and readers
impl<K: Key, V: Value, const IS_WRITER: bool> Map<K, V, IS_WRITER> {
	/// Get the number of entries in the map.
	///
	/// Time: O(1)
	pub fn len(&self) -> usize {
		self.data.borrow().len()
	}

	/// Test whether the map is empty.
	///
	/// Time: O(1)
	pub fn is_empty(&self) -> bool {
		self.data.borrow().is_empty()
	}

	/// Test whether the map contains a given key.
	///
	/// Time: O(log n)
	pub fn contains_key(&self, key: &K) -> bool {
		self.data.borrow().contains_key(key)
	}

	/// Get the value for a given key, if it exists.
	///
	/// Time: O(log n)
	pub fn get(&self, key: &K) -> Option<V> {
		self.data.borrow().get(key).cloned()
	}

	/// Get an iterator over the key-value pairs in the map.
	pub fn iter(&self) -> impl Iterator<Item = (K, V)> {
		let iter_clone = self.data.borrow().clone();
		iter_clone.into_iter()
	}

	/// Get an iterator over the keys in the map.
	pub fn keys(&self) -> impl Iterator<Item = K> {
		let iter_clone = self.data.borrow().clone();
		iter_clone.into_iter().map(|(k, _)| k)
	}

	/// Get an iterator over the values in the map.
	pub fn values(&self) -> impl Iterator<Item = V> {
		let iter_clone = self.data.borrow().clone();
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
	///
	/// This creates a new map with default synchronization configuration. If you
	/// want to customize the synchronization behavior (e.g. snapshot sync
	/// configuration), use `writer_with_config` instead.
	///
	/// Note that different sync configurations will create different group ids
	/// and the resulting maps will not be able to see each other.
	pub fn writer(network: &Network, store_id: StoreId) -> MapWriter<K, V> {
		Self::writer_with_config(network, store_id, SyncConfig::default())
	}

	/// Create a new map in writer mode.
	///
	/// The returned writer can be used to modify the map, and it also provides
	/// read access to the map's contents. Writers can be used by multiple
	/// nodes concurrently, and all changes made by any writer will be replicated
	/// to all other writers and readers.
	///
	/// This creates a new map with the specified sync configuration. If you
	/// want to use the default sync configuration, use the `writer` method
	/// instead.
	///
	/// Note that different sync configurations will create different group ids
	/// and the resulting maps will not be able to see each other.
	pub fn writer_with_config(
		network: &Network,
		store_id: StoreId,
		config: SyncConfig,
	) -> MapWriter<K, V> {
		Self::create::<WRITER>(network, store_id, config)
	}

	/// Create a new map in writer mode.
	///
	/// The returned writer can be used to modify the map, and it also provides
	/// read access to the map's contents. Writers can be used by multiple
	/// nodes concurrently, and all changes made by any writer will be replicated
	/// to all other writers and readers.
	///
	/// This creates a new map with default synchronization configuration. If you
	/// want to customize the synchronization behavior (e.g. snapshot sync
	/// configuration), use `writer_with_config` instead.
	///
	/// Note that different sync configurations will create different group ids
	/// and the resulting maps will not be able to see each other.
	pub fn new(network: &Network, store_id: StoreId) -> MapWriter<K, V> {
		Self::writer(network, store_id)
	}

	/// Create a new map in writer mode.
	///
	/// The returned writer can be used to modify the map, and it also provides
	/// read access to the map's contents. Writers can be used by multiple
	/// nodes concurrently, and all changes made by any writer will be replicated
	/// to all other writers and readers.
	///
	/// This creates a new map with the specified sync configuration. If you
	/// want to use the default sync configuration, use the `writer` method
	/// instead.
	///
	/// Note that different sync configurations will create different group ids
	/// and the resulting maps will not be able to see each other.
	pub fn new_with_config(
		network: &Network,
		store_id: StoreId,
		config: SyncConfig,
	) -> MapWriter<K, V> {
		Self::writer_with_config(network, store_id, config)
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
		Self::reader_with_config(network, store_id, SyncConfig::default())
	}

	/// Create a new map in reader mode.
	///
	/// The returned reader provides read-only access to the map's contents.
	/// Readers can be used by multiple nodes concurrently, and they will see all
	/// changes made by any writer. However, readers cannot modify the map, and
	/// they will not be able to make any changes themselves. Readers have longer
	/// election timeouts to reduce the likelihood of them being elected as
	/// group leaders, which reduces latency for read operations.
	pub fn reader_with_config(
		network: &Network,
		store_id: StoreId,
		config: SyncConfig,
	) -> MapReader<K, V> {
		Self::create::<READER>(network, store_id, config)
	}

	fn create<const W: bool>(
		network: &Network,
		store_id: StoreId,
		config: SyncConfig,
	) -> Map<K, V, W> {
		let machine = MapStateMachine::new(
			store_id, //
			W,
			config,
			network.local().id(),
		);

		let data = machine.data();
		let group = network
			.groups()
			.with_key(store_id)
			.with_state_machine(machine)
			.join();

		Map::<K, V, W> {
			when: When::new(group.when().clone()),
			group,
			data,
		}
	}
}

impl<K: Key, V: Value> SnapshotStateMachine for MapStateMachine<K, V> {
	type Snapshot = MapSnapshot<K, V>;

	fn create_snapshot(&self) -> Self::Snapshot {
		MapSnapshot {
			data: self.data.clone(),
		}
	}

	fn install_snapshot(&mut self, snapshot: Self::Snapshot) {
		self.data = snapshot.data;
		self.latest.send_replace(self.data.clone());
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
	data: HashMap<K, V>,
	latest: watch::Sender<HashMap<K, V>>,
	store_id: StoreId,
	local_id: PeerId,
	state_sync: SnapshotSync<Self>,
	is_writer: bool,
}

impl<K: Key, V: Value> MapStateMachine<K, V> {
	pub fn new(
		store_id: StoreId,
		is_writer: bool,
		sync_config: SyncConfig,
		local_id: PeerId,
	) -> Self {
		let data = HashMap::default();
		let state_sync = SnapshotSync::new(sync_config, |request| {
			MapCommand::TakeSnapshot(request)
		});

		let latest = watch::Sender::new(data.clone());

		Self {
			data,
			latest,
			store_id,
			local_id,
			state_sync,
			is_writer,
		}
	}

	pub fn data(&self) -> watch::Receiver<HashMap<K, V>> {
		self.latest.subscribe()
	}
}

impl<K: Key, V: Value> StateMachine for MapStateMachine<K, V> {
	type Command = MapCommand<K, V>;
	type Query = ();
	type QueryResult = ();
	type StateSync = SnapshotSync<Self>;

	fn apply(&mut self, command: Self::Command, ctx: &dyn ApplyContext) {
		self.apply_batch([command], ctx);
	}

	fn apply_batch(
		&mut self,
		commands: impl IntoIterator<Item = Self::Command>,
		ctx: &dyn ApplyContext,
	) {
		let mut commands_len = 0usize;
		let mut sync_requests = vec![];

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
				MapCommand::TakeSnapshot(request) => {
					if request.requested_by != self.local_id
						&& !self.state_sync.is_expired(&request)
					{
						// take note of the snapshot request, we will take the actual
						// snapshot after applying all commands from this batch.
						sync_requests.push(request);
					}
				}
			}
			commands_len += 1;
		}

		self.latest.send_replace(self.data.clone());

		if !sync_requests.is_empty() {
			let snapshot = self.create_snapshot();
			let position = Cursor::new(
				ctx.current_term(),
				ctx.committed().index() + commands_len as u64,
			);

			for request in sync_requests {
				self
					.state_sync
					.serve_snapshot(request, position, snapshot.clone());
			}
		}
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

	/// The state sync mechanism for out of sync followers.
	fn state_sync(&self) -> Self::StateSync {
		self.state_sync.clone()
	}

	/// Readers have longer election timeouts to reduce the likelihood of them
	/// being elected as group leaders.
	fn consensus_config(&self) -> Option<ConsensusConfig> {
		(!self.is_writer)
			.then(|| ConsensusConfig::default().deprioritize_leadership())
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "K: Key, V: Value")]
enum MapCommand<K, V> {
	Clear,
	Insert { key: K, value: V },
	Remove { key: K },
	Extend { entries: Vec<(K, V)> },
	TakeSnapshot(SnapshotRequest),
}

#[derive(Debug, Clone)]
pub struct MapSnapshot<K: Key, V: Value> {
	data: HashMap<K, V>,
}

impl<K: Key, V: Value> Default for MapSnapshot<K, V> {
	fn default() -> Self {
		Self {
			data: HashMap::default(),
		}
	}
}

impl<K: Key, V: Value> Snapshot for MapSnapshot<K, V> {
	type Item = (K, V);

	fn len(&self) -> u64 {
		self.data.len() as u64
	}

	fn iter_range(
		&self,
		range: std::ops::Range<u64>,
	) -> Option<impl Iterator<Item = Self::Item>> {
		if range.end > self.data.len() as u64 {
			return None;
		}

		Some(
			self
				.data
				.iter()
				.skip(range.start as usize)
				.take((range.end - range.start) as usize)
				.map(|(k, v)| (k.clone(), v.clone())),
		)
	}

	fn append(&mut self, items: impl IntoIterator<Item = Self::Item>) {
		self.data.extend(items);
	}
}
