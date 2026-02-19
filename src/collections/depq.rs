use {
	super::{
		Error,
		READER,
		WRITER,
		When,
		primitives::{Key, OrderedKey, StoreId, Value, Version},
	},
	crate::{
		Group,
		Network,
		UniqueId,
		groups::{CommandError, ConsensusConfig, LogReplaySync, StateMachine},
	},
	core::{any::type_name, ops::RangeBounds},
	serde::{Deserialize, Serialize},
	tokio::sync::watch,
};

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
	snapshot: watch::Receiver<DepqSnapshot<P, K, V>>,
}

// read-only access, available to both writers and readers
impl<P: OrderedKey, K: Key, V: Value, const IS_WRITER: bool>
	PriorityQueue<P, K, V, IS_WRITER>
{
	/// Get the number of entries in the priority queue.
	///
	/// Time: O(1)
	pub fn len(&self) -> usize {
		self.snapshot.borrow().by_key.len()
	}

	/// Test whether the priority queue is empty.
	///
	/// Time: O(1)
	pub fn is_empty(&self) -> bool {
		self.snapshot.borrow().by_key.is_empty()
	}

	/// Test whether the priority queue contains a given key.
	///
	/// Time: O(log n)
	pub fn contains_key(&self, key: &K) -> bool {
		self.snapshot.borrow().clone().by_key.contains_key(key)
	}

	/// Get the value for a given key, if it exists.
	///
	/// Time: O(log n)
	pub fn get(&self, key: &K) -> Option<V> {
		self
			.snapshot
			.borrow()
			.clone()
			.by_key
			.get(key)
			.map(|(_, v)| v.clone())
	}

	/// Get the priority for a given key, if it exists.
	///
	/// Time: O(log n)
	pub fn get_priority(&self, key: &K) -> Option<P> {
		self
			.snapshot
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
		let snap = self.snapshot.borrow().clone();
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
		let snap = self.snapshot.borrow().clone();
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
			.snapshot
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
			.snapshot
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
		let snap = self.snapshot.borrow().clone();
		snap.by_priority.into_iter().flat_map(|(p, bucket)| {
			bucket.into_iter().map(move |(k, v)| (p.clone(), k, v))
		})
	}

	/// Get an iterator over all entries in descending priority order.
	pub fn iter_desc(&self) -> impl Iterator<Item = (P, K, V)> {
		let snap = self.snapshot.borrow().clone();
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
					priority,
					key,
					value,
				},
				|cmd| match cmd {
					DepqCommand::Insert {
						priority,
						key,
						value,
					} => Error::Offline((priority, key, value)),
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
		let entries: Vec<(P, K, V)> = items.into_iter().collect();

		if entries.is_empty() {
			return Ok(Version(self.group.committed()));
		}

		self
			.execute(DepqCommand::Extend { entries }, |cmd| match cmd {
				DepqCommand::Extend { entries } => Error::Offline(entries),
				_ => unreachable!(),
			})
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
					key,
					priority: new_priority,
				},
				|cmd| match cmd {
					DepqCommand::UpdatePriority { key, .. } => Error::Offline(key),
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
					key,
					value: new_value,
				},
				|cmd| match cmd {
					DepqCommand::UpdateValue { key, .. } => Error::Offline(key),
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
			.execute(DepqCommand::Clear, |_| Error::Offline(()))
			.await
	}

	/// Remove an entry by key.
	///
	/// Time: O(log n)
	pub async fn remove(&self, key: &K) -> Result<Version, Error<K>> {
		let key = key.clone();
		self
			.execute(DepqCommand::Remove { key }, |cmd| match cmd {
				DepqCommand::Remove { key } => Error::Offline(key),
				_ => unreachable!(),
			})
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
		let start = SerBound::from_std(range.start_bound());
		let end = SerBound::from_std(range.end_bound());
		self
			.execute(DepqCommand::RemoveRange { start, end }, |_| {
				Error::Offline(())
			})
			.await
	}
}

// construction, available to both writers and readers
impl<P: OrderedKey, K: Key, V: Value, const IS_WRITER: bool>
	PriorityQueue<P, K, V, IS_WRITER>
{
	/// Create a new priority queue in writer mode.
	///
	/// The returned writer can be used to modify the queue, and it also provides
	/// read access to the queue's contents. Writers can be used by multiple
	/// nodes concurrently, and all changes made by any writer will be replicated
	/// to all other writers and readers.
	pub fn writer(
		network: &Network,
		store_id: StoreId,
	) -> PriorityQueueWriter<P, K, V> {
		let (machine, snapshot) = DepqStateMachine::new(store_id, true);
		let group = network
			.groups()
			.with_key(store_id.into())
			.with_state_machine(machine)
			.join();

		PriorityQueueWriter::<P, K, V> {
			when: When::new(group.when().clone()),
			group,
			snapshot,
		}
	}

	/// Create a new priority queue in writer mode.
	pub fn new(
		network: &Network,
		store_id: StoreId,
	) -> PriorityQueueWriter<P, K, V> {
		PriorityQueueWriter::<P, K, V>::writer(network, store_id)
	}

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
		store_id: StoreId,
	) -> PriorityQueueReader<P, K, V> {
		let (machine, snapshot) = DepqStateMachine::new(store_id, false);
		let group = network
			.groups()
			.with_key(store_id.into())
			.with_state_machine(machine)
			.join();

		PriorityQueueReader::<P, K, V> {
			when: When::new(group.when().clone()),
			group,
			snapshot,
		}
	}
}

// internal
impl<P: OrderedKey, K: Key, V: Value> PriorityQueueWriter<P, K, V> {
	async fn execute<TErr>(
		&self,
		command: DepqCommand<P, K, V>,
		offline_err: impl FnOnce(DepqCommand<P, K, V>) -> Error<TErr>,
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

struct DepqStateMachine<P: OrderedKey, K: Key, V: Value> {
	data: DepqSnapshot<P, K, V>,
	latest: watch::Sender<DepqSnapshot<P, K, V>>,
	store_id: StoreId,
	is_writer: bool,
}

impl<P: OrderedKey, K: Key, V: Value> DepqStateMachine<P, K, V> {
	pub fn new(
		store_id: StoreId,
		is_writer: bool,
	) -> (Self, watch::Receiver<DepqSnapshot<P, K, V>>) {
		let data = DepqSnapshot::new();
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
	type StateSync = LogReplaySync<Self>;

	fn apply(&mut self, command: Self::Command) {
		self.apply_batch([command]);
	}

	fn apply_batch(&mut self, commands: impl IntoIterator<Item = Self::Command>) {
		for command in commands {
			match command {
				DepqCommand::Clear => {
					self.data = DepqSnapshot::new();
				}
				DepqCommand::Insert {
					priority,
					key,
					value,
				} => {
					self.apply_insert(priority, key, value);
				}
				DepqCommand::Extend { entries } => {
					for (priority, key, value) in entries {
						self.apply_insert(priority, key, value);
					}
				}
				DepqCommand::UpdatePriority { key, priority } => {
					if let Some((_, old_v)) = self.data.by_key.get(&key) {
						let old_v = old_v.clone();
						self.apply_insert(priority, key, old_v);
					}
				}
				DepqCommand::UpdateValue { key, value } => {
					if let Some((p, _)) = self.data.by_key.get(&key) {
						let p = p.clone();
						self.apply_insert(p, key, value);
					}
				}
				DepqCommand::Remove { key } => {
					self.apply_remove(&key);
				}
				DepqCommand::RemoveRange { start, end } => {
					let range = (start.to_std(), end.to_std());
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
			}
		}
		self.latest.send_replace(self.data.clone());
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

	fn state_sync(&self) -> Self::StateSync {
		LogReplaySync::default()
	}

	fn consensus_config(&self) -> Option<ConsensusConfig> {
		let mut config = ConsensusConfig::default();

		if !self.is_writer {
			// Readers have longer election timeouts to reduce the likelihood of
			// them being elected as leaders. This is an optimization to reduce
			// latency.
			config.election_timeout *= 3;
			config.bootstrap_delay *= 3;
		}

		Some(config)
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "P: OrderedKey, K: Key, V: Value")]
enum DepqCommand<P, K, V> {
	Clear,
	Insert {
		priority: P,
		key: K,
		value: V,
	},
	Extend {
		entries: Vec<(P, K, V)>,
	},
	UpdatePriority {
		key: K,
		priority: P,
	},
	UpdateValue {
		key: K,
		value: V,
	},
	Remove {
		key: K,
	},
	RemoveRange {
		start: SerBound<P>,
		end: SerBound<P>,
	},
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
	fn from_std(b: core::ops::Bound<&T>) -> Self {
		match b {
			core::ops::Bound::Included(v) => Self::Included(v.clone()),
			core::ops::Bound::Excluded(v) => Self::Excluded(v.clone()),
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
#[derive(Clone)]
struct DepqSnapshot<P: OrderedKey, K: Key, V: Value> {
	/// Key → (priority, value) for O(log n) key lookups.
	by_key: im::HashMap<K, (P, V)>,
	/// Priority → {key → value} for O(log n) min/max and range operations.
	by_priority: im::OrdMap<P, im::HashMap<K, V>>,
}

impl<P: OrderedKey, K: Key, V: Value> DepqSnapshot<P, K, V> {
	fn new() -> Self {
		Self {
			by_key: im::HashMap::new(),
			by_priority: im::OrdMap::new(),
		}
	}
}
