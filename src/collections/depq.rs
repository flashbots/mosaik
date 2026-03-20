use {
	super::{
		CollectionFromDef,
		Error,
		READER,
		SyncConfig,
		WRITER,
		When,
		primitives::{Key, OrderedKey, StoreId, Value, Version},
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
			Cursor,
			LeadershipPreference,
			StateMachine,
		},
		primitives::{EncodeError, Encoded},
	},
	core::{
		any::type_name,
		borrow::Borrow,
		hash::Hash,
		ops::{Range, RangeBounds},
	},
	iroh::endpoint_info::EncodingError,
	serde::{Deserialize, Serialize},
	std::hash::BuildHasherDefault,
	tokio::sync::watch,
};

/// Deterministic hasher for the internal `im::HashMap`, ensuring that
/// iteration order is identical across all nodes for the same DEPQ state.
/// Uses `DefaultHasher` (SipHash-1-3) with a fixed zero seed.
type HashMap<K, V> =
	im::HashMap<K, V, BuildHasherDefault<std::hash::DefaultHasher>>;

pub type PriorityQueueWriter<P, K, V> = PriorityQueue<P, K, V, WRITER>;
pub type PriorityQueueReader<P, K, V> = PriorityQueue<P, K, V, READER>;

/// Replicated, eventually consistent double-ended priority queue (DEPQ).
///
/// A DEPQ is a collection of key-value pairs, each associated with a priority.
/// It supports efficient access to the minimum and maximum priority elements,
/// as well as key-based lookups, priority updates, and range removals.
///
/// Keys must be unique. Inserting a key that already exists will update its
/// priority and value.
pub struct PriorityQueue<
	P: OrderedKey,
	K: Key,
	V: Value,
	const IS_WRITER: bool = WRITER,
> {
	when: When,
	group: Group<DepqStateMachine<P, K, V>>,
	data: watch::Receiver<PriorityQueueSnapshot<P, K, V>>,
}

// read-only access, available to both writers and readers
impl<P: OrderedKey, K: Key, V: Value, const IS_WRITER: bool>
	PriorityQueue<P, K, V, IS_WRITER>
{
	/// Get the number of entries in the priority queue.
	///
	/// Time: O(1)
	pub fn len(&self) -> usize {
		self.data.borrow().by_key.len()
	}

	/// Test whether the priority queue is empty.
	///
	/// Time: O(1)
	pub fn is_empty(&self) -> bool {
		self.data.borrow().by_key.is_empty()
	}

	/// Test whether the priority queue contains a given key.
	///
	/// Time: O(log n)
	pub fn contains_key<Q>(&self, key: &Q) -> bool
	where
		K: Borrow<Q>,
		Q: Hash + Eq + ?Sized,
	{
		self.data.borrow().clone().by_key.contains_key(key)
	}

	/// Get the value for a given key, if it exists.
	///
	/// Time: O(log n)
	pub fn get<Q>(&self, key: &Q) -> Option<V>
	where
		K: Borrow<Q>,
		Q: Hash + Eq + ?Sized,
	{
		self
			.data
			.borrow()
			.clone()
			.by_key
			.get(key)
			.map(|(_, v)| v.clone())
	}

	/// Get the priority for a given key, if it exists.
	///
	/// Time: O(log n)
	pub fn get_priority<Q>(&self, key: &Q) -> Option<P>
	where
		K: Borrow<Q>,
		Q: Hash + Eq + ?Sized,
	{
		self
			.data
			.borrow()
			.clone()
			.by_key
			.get(key)
			.map(|(p, _)| p.clone())
	}

	/// Get the entry with the minimum priority.
	///
	/// If the queue is empty, `None` is returned. When multiple entries share
	/// the minimum priority, an arbitrary one among them is returned.
	///
	/// Time: O(log n)
	pub fn get_min(&self) -> Option<(P, K, V)> {
		let snap = self.data.borrow().clone();
		snap.by_priority.get_min().and_then(|(p, bucket)| {
			bucket
				.iter()
				.next()
				.map(|(k, v)| (p.clone(), k.clone(), v.clone()))
		})
	}

	/// Get the entry with the maximum priority.
	///
	/// If the queue is empty, `None` is returned. When multiple entries share
	/// the maximum priority, an arbitrary one among them is returned.
	///
	/// Time: O(log n)
	pub fn get_max(&self) -> Option<(P, K, V)> {
		let snap = self.data.borrow().clone();
		snap.by_priority.get_max().and_then(|(p, bucket)| {
			bucket
				.iter()
				.next()
				.map(|(k, v)| (p.clone(), k.clone(), v.clone()))
		})
	}

	/// Get the maximum priority in the queue, or `None` if empty.
	///
	/// Time: O(log n)
	pub fn max_priority(&self) -> Option<P> {
		self
			.data
			.borrow()
			.clone()
			.by_priority
			.get_max()
			.map(|(p, _)| p.clone())
	}

	/// Get the minimum priority in the queue, or `None` if empty.
	///
	/// Time: O(log n)
	pub fn min_priority(&self) -> Option<P> {
		self
			.data
			.borrow()
			.clone()
			.by_priority
			.get_min()
			.map(|(p, _)| p.clone())
	}

	/// Get an iterator over all entries in the priority queue, in ascending
	/// priority order.
	pub fn iter(&self) -> impl Iterator<Item = (P, K, V)> {
		self.iter_asc()
	}

	/// Get an iterator over all entries in ascending priority order.
	pub fn iter_asc(&self) -> impl Iterator<Item = (P, K, V)> {
		let snap = self.data.borrow().clone();
		snap.by_priority.into_iter().flat_map(|(p, bucket)| {
			bucket.into_iter().map(move |(k, v)| (p.clone(), k, v))
		})
	}

	/// Get an iterator over all entries in descending priority order.
	pub fn iter_desc(&self) -> impl Iterator<Item = (P, K, V)> {
		let snap = self.data.borrow().clone();
		let entries: std::vec::Vec<_> = snap
			.by_priority
			.into_iter()
			.rev()
			.flat_map(|(p, bucket)| {
				bucket.into_iter().map(move |(k, v)| (p.clone(), k, v))
			})
			.collect();
		entries.into_iter()
	}

	/// Returns an observer of the priority queue's state, which can be used to
	/// wait for the queue to reach a certain state version before performing an
	/// action or knowing when it is online or offline.
	pub const fn when(&self) -> &When {
		&self.when
	}

	/// The current version of the vector's state, which is the version of the
	/// latest committed state.
	pub fn version(&self) -> Version {
		Version(self.group.committed())
	}
}

// write access, only available to writers
impl<P: OrderedKey, K: Key, V: Value> PriorityQueueWriter<P, K, V> {
	/// Create a new priority queue in writer mode.
	///
	/// The returned writer can be used to modify the queue, and it also provides
	/// read access to the queue's contents. Writers can be used by multiple
	/// nodes concurrently, and all changes made by any writer will be replicated
	/// to all other writers and readers.
	///
	/// This creates a new priority queue with default synchronization
	/// configuration. If you want to customize the synchronization behavior
	/// (e.g. snapshot sync configuration), use `writer_with_config` instead.
	///
	/// Note that different sync configurations will create different group ids
	/// and the resulting queues will not be able to see each other.
	pub fn writer(network: &Network, store_id: impl Into<StoreId>) -> Self {
		Self::writer_with_config(network, store_id, SyncConfig::default())
	}

	/// Create a new priority queue in writer mode.
	///
	/// The returned writer can be used to modify the queue, and it also provides
	/// read access to the queue's contents. Writers can be used by multiple
	/// nodes concurrently, and all changes made by any writer will be replicated
	/// to all other writers and readers.
	///
	/// This creates a new priority queue with the specified sync configuration.
	/// If you want to use the default sync configuration, use the `writer`
	/// method instead.
	///
	/// Note that different sync configurations will create different group ids
	/// and the resulting queues will not be able to see each other.
	pub fn writer_with_config(
		network: &Network,
		store_id: impl Into<StoreId>,
		config: SyncConfig,
	) -> Self {
		Self::create::<WRITER>(network, store_id, config)
	}

	/// Create a new priority queue in writer mode.
	///
	/// This creates a new priority queue with default synchronization
	/// configuration. If you want to customize the synchronization behavior
	/// (e.g. snapshot sync configuration), use `new_with_config` instead.
	///
	/// Note that different sync configurations will create different group ids
	/// and the resulting queues will not be able to see each other.
	pub fn new(network: &Network, store_id: impl Into<StoreId>) -> Self {
		Self::writer(network, store_id)
	}

	/// Create a new priority queue in writer mode.
	///
	/// This creates a new priority queue with the specified sync configuration.
	/// If you want to use the default sync configuration, use the `new` method
	/// instead.
	///
	/// Note that different sync configurations will create different group ids
	/// and the resulting queues will not be able to see each other.
	pub fn new_with_config(
		network: &Network,
		store_id: impl Into<StoreId>,
		config: SyncConfig,
	) -> Self {
		Self::writer_with_config(network, store_id, config)
	}

	/// Insert an entry into the priority queue.
	///
	/// If the queue already contained this key, its priority and value are
	/// updated.
	///
	/// Time: O(log n)
	pub async fn insert(
		&self,
		priority: P,
		key: K,
		value: V,
	) -> Result<Version, Error<(P, K, V)>> {
		self
			.execute(
				DepqCommand::Insert {
					priority: Encoded(priority),
					key: Encoded(key),
					value: Encoded(value),
				},
				|cmd| match cmd {
					DepqCommand::Insert {
						priority,
						key,
						value,
					} => Error::Offline((priority.0, key.0, value.0)),
					_ => unreachable!(),
				},
				|cmd, e| match cmd {
					DepqCommand::Insert {
						priority,
						key,
						value,
					} => Error::Encoding((priority.0, key.0, value.0), e),
					_ => unreachable!(),
				},
			)
			.await
	}

	/// Insert multiple entries into the priority queue.
	pub async fn extend<I>(
		&self,
		items: I,
	) -> Result<Version, Error<Vec<(P, K, V)>>>
	where
		I: IntoIterator<Item = (P, K, V)>,
	{
		let entries: Vec<(Encoded<P>, Encoded<K>, Encoded<V>)> = items
			.into_iter()
			.map(|(p, k, v)| (Encoded(p), Encoded(k), Encoded(v)))
			.collect();

		if entries.is_empty() {
			return Ok(Version(self.group.committed()));
		}

		self
			.execute(
				DepqCommand::Extend { entries },
				|cmd| match cmd {
					DepqCommand::Extend { entries } => Error::Offline(
						entries
							.into_iter()
							.map(|(p, k, v)| (p.0, k.0, v.0))
							.collect(),
					),
					_ => unreachable!(),
				},
				|cmd, e| match cmd {
					DepqCommand::Extend { entries } => Error::Encoding(
						entries
							.into_iter()
							.map(|(p, k, v)| (p.0, k.0, v.0))
							.collect(),
						e,
					),
					_ => unreachable!(),
				},
			)
			.await
	}

	/// Update the priority of an existing key.
	///
	/// If the key does not exist, this is a no-op that still commits to the log.
	///
	/// Time: O(log n)
	pub async fn update_priority(
		&self,
		key: &K,
		new_priority: P,
	) -> Result<Version, Error<K>> {
		let key = key.clone();
		self
			.execute(
				DepqCommand::UpdatePriority {
					key: Encoded(key),
					priority: Encoded(new_priority),
				},
				|cmd| match cmd {
					DepqCommand::UpdatePriority { key, .. } => Error::Offline(key.0),
					_ => unreachable!(),
				},
				|cmd, e| match cmd {
					DepqCommand::UpdatePriority { key, .. } => Error::Encoding(key.0, e),
					_ => unreachable!(),
				},
			)
			.await
	}

	/// Update the value of an existing key.
	///
	/// If the key does not exist, this is a no-op that still commits to the log.
	///
	/// Time: O(log n)
	pub async fn update_value(
		&self,
		key: &K,
		new_value: V,
	) -> Result<Version, Error<K>> {
		let key = key.clone();
		self
			.execute(
				DepqCommand::UpdateValue {
					key: Encoded(key),
					value: Encoded(new_value),
				},
				|cmd| match cmd {
					DepqCommand::UpdateValue { key, .. } => Error::Offline(key.0),
					_ => unreachable!(),
				},
				|cmd, e| match cmd {
					DepqCommand::UpdateValue { key, .. } => Error::Encoding(key.0, e),
					_ => unreachable!(),
				},
			)
			.await
	}

	/// Atomically update the value of an existing key, but only if its current
	/// value matches the expected value. If the new value is `None`, the key is
	/// removed instead.
	///
	/// If the key does not exist, this is a no-op that still commits to the log.
	///
	/// Time: O(log n)
	pub async fn compare_exchange_value(
		&self,
		key: &K,
		expected: V,
		new: Option<V>,
	) -> Result<Version, Error<K>> {
		let key = key.clone();
		self
			.execute(
				DepqCommand::CompareExchangeValue {
					key: Encoded(key),
					expected: Encoded(expected),
					new: new.map(Encoded),
				},
				|cmd| match cmd {
					DepqCommand::CompareExchangeValue { key, .. } => {
						Error::Offline(key.0)
					}
					_ => unreachable!(),
				},
				|cmd, e| match cmd {
					DepqCommand::CompareExchangeValue { key, .. } => {
						Error::Encoding(key.0, e)
					}
					_ => unreachable!(),
				},
			)
			.await
	}

	/// Discard all entries from the priority queue.
	///
	/// This leaves you with an empty queue, and all entries that were previously
	/// inside it are dropped.
	pub async fn clear(&self) -> Result<Version, Error<()>> {
		self
			.execute(
				DepqCommand::Clear,
				|_| Error::Offline(()),
				|_, _| unreachable!(),
			)
			.await
	}

	/// Remove an entry by key.
	///
	/// Time: O(log n)
	pub async fn remove(&self, key: &K) -> Result<Version, Error<K>> {
		let key = key.clone();
		self
			.execute(
				DepqCommand::RemoveKeys {
					keys: vec![Encoded(key)],
				},
				|cmd| match cmd {
					DepqCommand::RemoveKeys { mut keys } => {
						Error::Offline(keys.remove(0).0)
					}
					_ => unreachable!(),
				},
				|cmd, e| match cmd {
					DepqCommand::RemoveKeys { mut keys } => {
						Error::Encoding(keys.remove(0).0, e)
					}
					_ => unreachable!(),
				},
			)
			.await
	}

	/// Remove multiple entries by key.
	pub async fn remove_keys(
		&self,
		keys: impl IntoIterator<Item = K>,
	) -> Result<Version, Error<Vec<K>>> {
		let keys: Vec<Encoded<K>> = keys.into_iter().map(Encoded).collect();

		if keys.is_empty() {
			return Ok(Version(self.group.committed()));
		}

		self
			.execute(
				DepqCommand::RemoveKeys { keys },
				|cmd| match cmd {
					DepqCommand::RemoveKeys { keys } => {
						Error::Offline(keys.into_iter().map(|k| k.0).collect())
					}
					_ => unreachable!(),
				},
				|cmd, e| match cmd {
					DepqCommand::RemoveKeys { keys } => {
						Error::Encoding(keys.into_iter().map(|k| k.0).collect(), e)
					}
					_ => unreachable!(),
				},
			)
			.await
	}

	/// Remove all entries whose priority falls within the given range.
	///
	/// Accepts any `RangeBounds<P>`, so all standard range syntaxes work:
	///
	/// - `remove_range(..cutoff)` — remove priorities below `cutoff`
	/// - `remove_range(..=cutoff)` — remove priorities at or below `cutoff`
	/// - `remove_range(cutoff..)` — remove priorities at or above `cutoff`
	/// - `remove_range(lo..hi)` — remove priorities in `[lo, hi)`
	/// - `remove_range(lo..=hi)` — remove priorities in `[lo, hi]`
	/// - `remove_range(..)` — equivalent to `clear()`
	pub async fn remove_range(
		&self,
		range: impl RangeBounds<P>,
	) -> Result<Version, Error<()>> {
		let start =
			SerBound::from_std(range.start_bound().map(|r| Encoded(r.clone())));
		let end = SerBound::from_std(range.end_bound().map(|r| Encoded(r.clone())));
		self
			.execute(
				DepqCommand::RemoveRange { start, end },
				|_| Error::Offline(()),
				|_, e| Error::Encoding((), e),
			)
			.await
	}
}

// construction, available to both writers and readers
impl<P: OrderedKey, K: Key, V: Value, const IS_WRITER: bool>
	PriorityQueue<P, K, V, IS_WRITER>
{
	/// Create a new priority queue in reader mode.
	///
	/// The returned reader provides read-only access to the queue's contents.
	/// Readers can be used by multiple nodes concurrently, and they will see all
	/// changes made by any writer. However, readers cannot modify the queue, and
	/// they will not be able to make any changes themselves. Readers have longer
	/// election timeouts to reduce the likelihood of them being elected as
	/// group leaders, which reduces latency for read operations.
	pub fn reader(
		network: &Network,
		store_id: impl Into<StoreId>,
	) -> PriorityQueueReader<P, K, V> {
		Self::reader_with_config(network, store_id, SyncConfig::default())
	}

	/// Create a new priority queue in reader mode.
	///
	/// The returned reader provides read-only access to the queue's contents.
	/// Readers can be used by multiple nodes concurrently, and they will see all
	/// changes made by any writer. However, readers cannot modify the queue, and
	/// they will not be able to make any changes themselves. Readers have longer
	/// election timeouts to reduce the likelihood of them being elected as
	/// group leaders, which reduces latency for read operations.
	pub fn reader_with_config(
		network: &Network,
		store_id: impl Into<StoreId>,
		config: SyncConfig,
	) -> PriorityQueueReader<P, K, V> {
		Self::create::<READER>(network, store_id, config)
	}

	fn create<const W: bool>(
		network: &Network,
		store_id: impl Into<StoreId>,
		config: SyncConfig,
	) -> PriorityQueue<P, K, V, W> {
		let store_id = store_id.into();
		let machine = DepqStateMachine::new(
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

		PriorityQueue::<P, K, V, W> {
			when: When::new(group.when().clone()),
			group,
			data,
		}
	}
}

impl<P: OrderedKey, K: Key, V: Value, const WRITER: bool> CollectionFromDef
	for PriorityQueue<P, K, V, WRITER>
{
	type Reader = PriorityQueueReader<P, K, V>;
	type Writer = PriorityQueueWriter<P, K, V>;

	fn reader(network: &crate::Network, store_id: StoreId) -> Self::Reader {
		Self::Reader::reader(network, store_id)
	}

	fn writer(network: &crate::Network, store_id: StoreId) -> Self::Writer {
		Self::Writer::writer(network, store_id)
	}
}

// internal
impl<P: OrderedKey, K: Key, V: Value> PriorityQueueWriter<P, K, V> {
	async fn execute<TErr>(
		&self,
		command: DepqCommand<P, K, V>,
		offline_err: impl FnOnce(DepqCommand<P, K, V>) -> Error<TErr>,
		encoding_err: impl FnOnce(DepqCommand<P, K, V>, EncodeError) -> Error<TErr>,
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
				CommandError::Encoding(mut items, err) => {
					let command = items.remove(0);
					encoding_err(command, err)
				}
				CommandError::GroupTerminated => Error::NetworkDown,
				CommandError::NoCommands => unreachable!(),
			})
	}
}

struct DepqStateMachine<P: OrderedKey, K: Key, V: Value> {
	data: PriorityQueueSnapshot<P, K, V>,
	latest: watch::Sender<PriorityQueueSnapshot<P, K, V>>,
	store_id: StoreId,
	local_id: PeerId,
	state_sync: SnapshotSync<Self>,
	is_writer: bool,
}

impl<P: OrderedKey, K: Key, V: Value> DepqStateMachine<P, K, V> {
	pub fn new(
		store_id: StoreId,
		is_writer: bool,
		sync_config: SyncConfig,
		local_id: PeerId,
	) -> Self {
		let data = PriorityQueueSnapshot::default();
		let state_sync = SnapshotSync::new(sync_config, |request| {
			DepqCommand::TakeSnapshot(request)
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

	pub fn data(&self) -> watch::Receiver<PriorityQueueSnapshot<P, K, V>> {
		self.latest.subscribe()
	}

	/// Insert a single entry, updating both indexes. If the key already exists,
	/// removes it from the old priority bucket first.
	fn apply_insert(&mut self, priority: P, key: K, value: V) {
		// Remove from old priority bucket if key already exists
		if let Some((old_p, _)) = self.data.by_key.get(&key) {
			let old_p = old_p.clone();
			if let Some(bucket) = self.data.by_priority.get(&old_p) {
				let mut bucket = bucket.clone();
				bucket.remove(&key);
				if bucket.is_empty() {
					self.data.by_priority.remove(&old_p);
				} else {
					self.data.by_priority.insert(old_p, bucket);
				}
			}
		}

		// Insert into both indexes
		self
			.data
			.by_key
			.insert(key.clone(), (priority.clone(), value.clone()));
		let bucket = self
			.data
			.by_priority
			.get(&priority)
			.cloned()
			.unwrap_or_default();
		let mut bucket = bucket;
		bucket.insert(key, value);
		self.data.by_priority.insert(priority, bucket);
	}

	/// Remove an entry by key from both indexes.
	fn apply_remove(&mut self, key: &K) {
		if let Some((p, _)) = self.data.by_key.remove(key)
			&& let Some(bucket) = self.data.by_priority.get(&p)
		{
			let mut bucket = bucket.clone();
			bucket.remove(key);
			if bucket.is_empty() {
				self.data.by_priority.remove(&p);
			} else {
				self.data.by_priority.insert(p, bucket);
			}
		}
	}
}

impl<P: OrderedKey, K: Key, V: Value> StateMachine
	for DepqStateMachine<P, K, V>
{
	type Command = DepqCommand<P, K, V>;
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
				DepqCommand::Clear => {
					self.data = PriorityQueueSnapshot::default();
				}
				DepqCommand::Insert {
					priority,
					key,
					value,
				} => {
					self.apply_insert(priority.0, key.0, value.0);
				}
				DepqCommand::CompareExchangeValue { key, expected, new } => {
					if let Some((p, old_v)) = self.data.by_key.get(&key) {
						if old_v.encode().ok() != expected.0.encode().ok() {
							continue;
						}
						let p = p.clone();
						if let Some(new) = new {
							self.apply_insert(p, key.0, new.0);
						} else {
							self.apply_remove(&key.0);
						}
					}
				}
				DepqCommand::Extend { entries } => {
					for (priority, key, value) in entries {
						self.apply_insert(priority.0, key.0, value.0);
					}
				}
				DepqCommand::UpdatePriority { key, priority } => {
					if let Some((_, old_v)) = self.data.by_key.get(&key) {
						let old_v = old_v.clone();
						self.apply_insert(priority.0, key.0, old_v);
					}
				}
				DepqCommand::UpdateValue { key, value } => {
					if let Some((p, _)) = self.data.by_key.get(&key) {
						let p = p.clone();
						self.apply_insert(p, key.0, value.0);
					}
				}
				DepqCommand::RemoveKeys { keys } => {
					for key in keys {
						self.apply_remove(&key.0);
					}
				}
				DepqCommand::RemoveRange { start, end } => {
					let range = (
						start.to_std().map(|r| r.0.clone()),
						end.to_std().map(|r| r.0.clone()),
					);

					let keys_to_remove: std::vec::Vec<K> = self
						.data
						.by_key
						.iter()
						.filter(|(_, (p, _))| range.contains(p))
						.map(|(k, _)| k.clone())
						.collect();

					for key in keys_to_remove {
						self.apply_remove(&key);
					}
				}
				DepqCommand::TakeSnapshot(request) => {
					if request.requested_by != self.local_id
						&& !self.state_sync.is_expired(&request)
					{
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

	/// The group-key for a DEPQ is derived from the store ID and the types of
	/// the queue's priorities, keys, and values. This ensures that different
	/// queues (with different store IDs or type parameters) will be in different
	/// groups.
	fn signature(&self) -> crate::UniqueId {
		UniqueId::from("mosaik_collections_depq")
			.derive(self.store_id)
			.derive(type_name::<P>())
			.derive(type_name::<K>())
			.derive(type_name::<V>())
	}

	/// This state machine doesn't support external queries, because all of its
	/// state is observable through the `latest` watch channel. Therefore, the
	/// query method is a no-op.
	fn query(&self, (): Self::Query) {}

	/// This state machine uses the `SnapshotSync` state sync strategy.
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

impl<P: OrderedKey, K: Key, V: Value> SnapshotStateMachine
	for DepqStateMachine<P, K, V>
{
	type Snapshot = PriorityQueueSnapshot<P, K, V>;

	fn create_snapshot(&self) -> Self::Snapshot {
		PriorityQueueSnapshot {
			by_key: self.data.by_key.clone(),
			by_priority: self.data.by_priority.clone(),
		}
	}

	fn install_snapshot(&mut self, snapshot: Self::Snapshot) {
		self.data = snapshot;
		self.latest.send_replace(self.data.clone());
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "P: OrderedKey, K: Key, V: Value")]
enum DepqCommand<P, K, V> {
	Clear,
	Insert {
		priority: Encoded<P>,
		key: Encoded<K>,
		value: Encoded<V>,
	},
	Extend {
		entries: Vec<(Encoded<P>, Encoded<K>, Encoded<V>)>,
	},
	UpdatePriority {
		key: Encoded<K>,
		priority: Encoded<P>,
	},
	UpdateValue {
		key: Encoded<K>,
		value: Encoded<V>,
	},
	CompareExchangeValue {
		key: Encoded<K>,
		expected: Encoded<V>,
		new: Option<Encoded<V>>,
	},
	RemoveKeys {
		keys: Vec<Encoded<K>>,
	},
	RemoveRange {
		start: SerBound<Encoded<P>>,
		end: SerBound<Encoded<P>>,
	},
	TakeSnapshot(SnapshotRequest),
}

/// Serializable equivalent of [`core::ops::Bound`], needed because
/// `Bound<T>` doesn't implement `Serialize` / `Deserialize`.
#[derive(Debug, Clone, Serialize, Deserialize)]
enum SerBound<T> {
	Included(T),
	Excluded(T),
	Unbounded,
}

impl<T: Clone> SerBound<T> {
	fn from_std(b: core::ops::Bound<T>) -> Self {
		match b {
			core::ops::Bound::Included(v) => Self::Included(v),
			core::ops::Bound::Excluded(v) => Self::Excluded(v),
			core::ops::Bound::Unbounded => Self::Unbounded,
		}
	}

	const fn to_std(&self) -> core::ops::Bound<&T> {
		match self {
			Self::Included(v) => core::ops::Bound::Included(v),
			Self::Excluded(v) => core::ops::Bound::Excluded(v),
			Self::Unbounded => core::ops::Bound::Unbounded,
		}
	}
}

/// Snapshot of the DEPQ state, combining both indexes for efficient access.
///
/// Only the `by_key` index is synced over the wire because `by_priority` can
/// be fully reconstructed from the `(P, K, V)` tuples stored in `by_key`.
/// Both indexes are maintained incrementally during `append` so that
/// `install_snapshot` is a cheap move rather than one large rebuild.
#[derive(Clone)]
pub struct PriorityQueueSnapshot<P: OrderedKey, K: Key, V: Value> {
	/// Key → (priority, value) for O(log n) key lookups.
	by_key: HashMap<K, (P, V)>,
	/// Priority → {key → value} for O(log n) min/max and range operations.
	by_priority: im::OrdMap<P, HashMap<K, V>>,
}

impl<P: OrderedKey, K: Key, V: Value> Default
	for PriorityQueueSnapshot<P, K, V>
{
	fn default() -> Self {
		Self {
			by_key: HashMap::default(),
			by_priority: im::OrdMap::new(),
		}
	}
}

impl<P: OrderedKey, K: Key, V: Value> Snapshot
	for PriorityQueueSnapshot<P, K, V>
{
	type Item = (Encoded<P>, Encoded<K>, Encoded<V>);

	fn len(&self) -> u64 {
		self.by_key.len() as u64
	}

	fn iter_range(
		&self,
		range: Range<u64>,
	) -> Option<impl Iterator<Item = Self::Item>> {
		if range.end > self.by_key.len() as u64 {
			return None;
		}

		Some(
			self
				.by_key
				.clone()
				.into_iter()
				.map(|(k, (p, v))| (Encoded(p), Encoded(k), Encoded(v)))
				.skip(range.start as usize)
				.take((range.end - range.start) as usize),
		)
	}

	fn append(&mut self, items: impl IntoIterator<Item = Self::Item>) {
		for (priority, key, value) in items {
			let (priority, key, value) = (priority.0, key.0, value.0);

			self
				.by_key
				.insert(key.clone(), (priority.clone(), value.clone()));
			let mut bucket =
				self.by_priority.get(&priority).cloned().unwrap_or_default();
			bucket.insert(key, value);
			self.by_priority.insert(priority, bucket);
		}
	}
}
