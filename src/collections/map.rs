use {
	super::{
		CollectionConfig,
		Error,
		READER,
		SyncConfig,
		WRITER,
		When,
		primitives::{Key, StoreId, Value, Version},
	},
	crate::{
		Group,
		GroupId,
		Network,
		PeerId,
		UniqueId,
		collections::{
			CollectionFromDef,
			sync::{
				Snapshot,
				SnapshotStateMachine,
				SnapshotSync,
				protocol::SnapshotRequest,
			},
		},
		groups::{
			ApplyContext,
			CommandError,
			Cursor,
			LeadershipPreference,
			LogReplaySync,
			StateMachine,
		},
		primitives::{EncodeError, Encoded, ShortFmtExt},
	},
	core::{any::type_name, borrow::Borrow, hash::Hash},
	futures::{FutureExt, TryFutureExt},
	serde::{Deserialize, Serialize},
	std::{hash::BuildHasherDefault, sync::OnceLock},
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
	pub fn contains_key<Q>(&self, key: &Q) -> bool
	where
		K: Borrow<Q>,
		Q: Hash + Eq + ?Sized,
	{
		self.data.borrow().contains_key(key)
	}

	/// Get the value for a given key, if it exists.
	///
	/// Time: O(log n)
	pub fn get<Q>(&self, key: &Q) -> Option<V>
	where
		K: Borrow<Q>,
		Q: Hash + Eq + ?Sized,
	{
		self.data.borrow().get(key.borrow()).cloned()
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

	/// The current version of the map's state, which is the version of the
	/// latest committed state.
	pub fn version(&self) -> Version {
		Version(self.group.committed())
	}

	/// The group id of the underlying consensus group for this collection
	/// instance.
	pub fn group_id(&self) -> &GroupId {
		self.group.id()
	}
}

// Mutable operations, only available to writers
impl<K: Key, V: Value> MapWriter<K, V> {
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
	pub fn writer(network: &Network, store_id: impl Into<StoreId>) -> Self {
		Self::writer_with_config(network, store_id, CollectionConfig::default())
	}

	/// Create a new map in writer mode.
	///
	/// The returned writer can be used to modify the map, and it also provides
	/// read access to the map's contents. Writers can be used by multiple
	/// nodes concurrently, and all changes made by any writer will be replicated
	/// to all other writers and readers.
	///
	/// This creates a new map with the specified configuration. If you
	/// want to use the default configuration, use the `writer` method
	/// instead.
	///
	/// Note that different configurations will create different group ids
	/// and the resulting maps will not be able to see each other.
	pub fn writer_with_config(
		network: &Network,
		store_id: impl Into<StoreId>,
		config: impl Into<CollectionConfig>,
	) -> Self {
		Self::create::<WRITER>(network, store_id, config.into())
	}

	/// Create a new map in writer mode.
	///
	/// This is an alias for the `writer` method.
	pub fn new(network: &Network, store_id: impl Into<StoreId>) -> Self {
		Self::writer(network, store_id)
	}

	/// Create a new map in writer mode with the specified configuration.
	///
	/// This is an alias for the `writer_with_config` method.
	pub fn new_with_config(
		network: &Network,
		store_id: impl Into<StoreId>,
		config: impl Into<CollectionConfig>,
	) -> Self {
		Self::writer_with_config(network, store_id, config)
	}

	/// Discard all entries from the map.
	///
	/// This leaves you with an empty map, and all entries that
	/// were previously inside it are dropped.
	pub fn clear(
		&self,
	) -> impl Future<Output = Result<Version, Error<()>>> + Send + Sync + 'static
	{
		self.execute(
			MapCommand::Clear,
			|_| Error::Offline(()),
			|_, _| unreachable!(),
		)
	}

	/// Insert a key-value pair into the map.
	///
	/// If the map already contained this key, the value is updated.
	///
	/// Time: O(log n)
	pub fn insert(
		&self,
		key: K,
		value: V,
	) -> impl Future<Output = Result<Version, Error<(K, V)>>> + Send + Sync + 'static
	{
		let key = Encoded(key);
		let value = Encoded(value);

		self.execute(
			MapCommand::Insert { key, value },
			|cmd| match cmd {
				MapCommand::Insert { key, value } => Error::Offline((key.0, value.0)),
				_ => unreachable!(),
			},
			|cmd, e| match cmd {
				MapCommand::Insert { key, value } => {
					Error::Encoding((key.0, value.0), e)
				}
				_ => unreachable!(),
			},
		)
	}

	/// Compare and exchange a key-value pair in the map.
	///
	/// This will update the value for the given key to `new` if and only if the
	/// current value matches `expected`. If the key does not exist, the current
	/// value is considered to be `None`, so you can use this method to insert a
	/// new key-value pair by setting `expected` to `None` and `new` to
	/// `Some(value)`. Conversely, you can delete a key-value pair by setting
	/// `expected` to `Some(value)` and `new` to `None`.
	///
	/// Time: O(log n)
	#[allow(clippy::type_complexity)]
	pub fn compare_exchange(
		&self,
		key: K,
		expected: Option<V>,
		new: Option<V>,
	) -> impl Future<Output = Result<Version, Error<(K, Option<V>, Option<V>)>>>
	+ Send
	+ Sync
	+ 'static {
		let key = Encoded(key);
		let expected = expected.map(Encoded);
		let new = new.map(Encoded);

		self.execute(
			MapCommand::CompareExchange { key, expected, new },
			|cmd| match cmd {
				MapCommand::CompareExchange { key, expected, new } => {
					Error::Offline((key.0, expected.map(|v| v.0), new.map(|v| v.0)))
				}
				_ => unreachable!(),
			},
			|cmd, e| match cmd {
				MapCommand::CompareExchange { key, expected, new } => {
					Error::Encoding((key.0, expected.map(|v| v.0), new.map(|v| v.0)), e)
				}
				_ => unreachable!(),
			},
		)
	}

	/// Insert multiple key-value pairs into the map.
	pub fn extend(
		&self,
		entries: impl IntoIterator<Item = (K, V)>,
	) -> impl Future<Output = Result<Version, Error<Vec<(K, V)>>>>
	+ Send
	+ Sync
	+ 'static {
		let entries: Vec<(Encoded<K>, Encoded<V>)> = entries
			.into_iter()
			.map(|(k, v)| (Encoded(k), Encoded(v)))
			.collect();

		let is_empty = entries.is_empty();
		let current_version = self.group.committed();
		let fut = self.execute(
			MapCommand::Extend { entries },
			|cmd| match cmd {
				MapCommand::Extend { entries } => {
					Error::Offline(entries.into_iter().map(|(k, v)| (k.0, v.0)).collect())
				}
				_ => unreachable!(),
			},
			|cmd, e| match cmd {
				MapCommand::Extend { entries } => Error::Encoding(
					entries.into_iter().map(|(k, v)| (k.0, v.0)).collect(),
					e,
				),
				_ => unreachable!(),
			},
		);

		async move {
			if is_empty {
				Ok(Version(current_version))
			} else {
				fut.await
			}
		}
	}

	/// Remove a key-value pair from the map.
	///
	/// Time: O(log n)
	pub fn remove(
		&self,
		key: &K,
	) -> impl Future<Output = Result<Version, Error<K>>> + Send + Sync + 'static
	{
		let key = Encoded(key.clone());
		self.execute(
			MapCommand::RemoveMany { keys: vec![key] },
			|cmd| match cmd {
				MapCommand::RemoveMany { mut keys } => Error::Offline(keys.remove(0).0),
				_ => unreachable!(),
			},
			|cmd, e| match cmd {
				MapCommand::RemoveMany { mut keys } => {
					Error::Encoding(keys.remove(0).0, e)
				}
				_ => unreachable!(),
			},
		)
	}

	/// Remove multiple key-value pairs from the map.
	pub fn remove_many(
		&self,
		keys: impl IntoIterator<Item = K>,
	) -> impl Future<Output = Result<Version, Error<Vec<K>>>> + Send + Sync + 'static
	{
		let keys: Vec<Encoded<K>> = keys.into_iter().map(Encoded).collect();

		let is_empty = keys.is_empty();
		let current_version = self.group.committed();

		let fut = self.execute(
			MapCommand::RemoveMany { keys },
			|cmd| match cmd {
				MapCommand::RemoveMany { keys } => {
					Error::Offline(keys.into_iter().map(|k| k.0).collect())
				}
				_ => unreachable!(),
			},
			|cmd, e| match cmd {
				MapCommand::RemoveMany { keys } => {
					Error::Encoding(keys.into_iter().map(|k| k.0).collect(), e)
				}
				_ => unreachable!(),
			},
		);

		async move {
			if is_empty {
				Ok(Version(current_version))
			} else {
				fut.await
			}
		}
	}
}

impl<K: Key, V: Value, const WRITER: bool> CollectionFromDef
	for Map<K, V, WRITER>
{
	type Reader = MapReader<K, V>;
	type Writer = MapWriter<K, V>;

	fn reader_with_config(
		network: &crate::Network,
		store_id: StoreId,
		config: CollectionConfig,
	) -> Self::Reader {
		Self::Reader::reader_with_config(network, store_id, config)
	}

	fn writer_with_config(
		network: &crate::Network,
		store_id: StoreId,
		config: CollectionConfig,
	) -> Self::Writer {
		Self::Writer::writer_with_config(network, store_id, config)
	}
}

// construction
impl<K: Key, V: Value, const IS_WRITER: bool> Map<K, V, IS_WRITER> {
	/// Create a new map in reader mode.
	pub fn reader(
		network: &Network,
		store_id: impl Into<StoreId>,
	) -> MapReader<K, V> {
		Self::reader_with_config(network, store_id, CollectionConfig::default())
	}

	/// Create a new map in reader mode with the specified configuration.
	pub fn reader_with_config(
		network: &Network,
		store_id: impl Into<StoreId>,
		config: impl Into<CollectionConfig>,
	) -> MapReader<K, V> {
		Self::create::<READER>(network, store_id, config.into())
	}

	fn create<const W: bool>(
		network: &Network,
		store_id: impl Into<StoreId>,
		config: CollectionConfig,
	) -> Map<K, V, W> {
		let store_id = store_id.into();
		let machine = MapStateMachine::new(
			store_id, //
			W,
			config.sync,
			network.local().id(),
		);

		let data = machine.data();
		let mut builder = network
			.groups()
			.with_key(store_id)
			.with_state_machine(machine);

		for validator in config.auth {
			builder = builder.require_ticket(validator);
		}

		let group = builder.join();

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
	fn execute<TErr>(
		&self,
		command: MapCommand<K, V>,
		offline_err: impl FnOnce(MapCommand<K, V>) -> Error<TErr>
		+ Send
		+ Sync
		+ 'static,
		encoding_err: impl FnOnce(MapCommand<K, V>, EncodeError) -> Error<TErr>
		+ Send
		+ Sync
		+ 'static,
	) -> impl Future<Output = Result<Version, Error<TErr>>> + Send + Sync + 'static
	{
		self
			.group
			.execute(command)
			.map_err(move |e| match e {
				CommandError::Offline(mut items) => {
					let command = items.remove(0);
					offline_err(command)
				}
				CommandError::Encoding(mut items, err) => {
					let command = items.remove(0);
					encoding_err(command, err)
				}
				CommandError::GroupTerminated => Error::NetworkDown,
				CommandError::NoCommands => unreachable!(),
			})
			.map(move |position| position.map(Version))
	}
}

struct MapStateMachine<K: Key, V: Value> {
	data: HashMap<K, V>,
	latest: watch::Sender<HashMap<K, V>>,
	store_id: StoreId,
	local_id: PeerId,
	state_sync: SnapshotSync<Self>,
	is_writer: bool,
	metrics_labels: OnceLock<[(&'static str, String); 2]>,
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
			metrics_labels: OnceLock::new(),
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
					self.data.insert(key.0, value.0);
				}
				MapCommand::CompareExchange { key, expected, new } => {
					match (self.data.get(&key.0), expected) {
						(Some(current), Some(expected))
							if current.encode().ok() == expected.encode().ok() =>
						{
							if let Some(new) = new {
								self.data.insert(key.0, new.0);
							} else {
								self.data.remove(&key.0);
							}
						}
						(None, None) => {
							if let Some(new) = new {
								self.data.insert(key.0, new.0);
							}
						}
						_ => {}
					}
				}
				MapCommand::RemoveMany { keys } => {
					for key in keys {
						self.data.remove(&key.0);
					}
				}
				MapCommand::Extend { entries } => {
					for (key, value) in entries {
						self.data.insert(key.0, value.0);
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

		let labels = self.metrics_labels.get_or_init(|| {
			[
				("network", ctx.network_id().short().to_string()),
				("group", ctx.group_id().short().to_string()),
			]
		});
		metrics::gauge!("mosaik.collections.map.size", labels.as_slice())
			.set(self.data.len() as f64);

		if !sync_requests.is_empty() {
			let snapshot = self.create_snapshot();
			let position = Cursor::new(
				ctx.current_term(),
				ctx.committed().index() + commands_len as u64,
			);

			metrics::counter!("mosaik.collections.syncs.started", labels.as_slice())
				.increment(sync_requests.len() as u64);
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

	/// Readers are observers and never assume group leadership.
	fn leadership_preference(&self) -> LeadershipPreference {
		if self.is_writer {
			LeadershipPreference::Normal
		} else {
			LeadershipPreference::Observer
		}
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "K: Key, V: Value")]
enum MapCommand<K, V> {
	Clear,
	Insert {
		key: Encoded<K>,
		value: Encoded<V>,
	},
	CompareExchange {
		key: Encoded<K>,
		expected: Option<Encoded<V>>,
		new: Option<Encoded<V>>,
	},
	RemoveMany {
		keys: Vec<Encoded<K>>,
	},
	Extend {
		entries: Vec<(Encoded<K>, Encoded<V>)>,
	},
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
	type Item = (Encoded<K>, Encoded<V>);

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
				.clone()
				.into_iter()
				.skip(range.start as usize)
				.take((range.end - range.start) as usize)
				.map(|(k, v)| (Encoded(k), Encoded(v))),
		)
	}

	fn append(&mut self, items: impl IntoIterator<Item = Self::Item>) {
		self.data.extend(items.into_iter().map(|(k, v)| (k.0, v.0)));
	}
}
