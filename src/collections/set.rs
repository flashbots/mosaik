use {
	super::{
		Error,
		READER,
		SyncConfig,
		WRITER,
		When,
		primitives::{Key, StoreId, Version},
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
			StateMachine,
		},
	},
	core::{any::type_name, ops::Range},
	serde::{Deserialize, Serialize},
	std::hash::BuildHasherDefault,
	tokio::sync::watch,
};

/// Deterministic hasher for the internal `im::HashSet`, ensuring that
/// iteration order is identical across all nodes for the same set state.
/// Uses `DefaultHasher` (SipHash-1-3) with a fixed zero seed.
type HashSet<T> = im::HashSet<T, BuildHasherDefault<std::hash::DefaultHasher>>;

pub type SetWriter<T> = Set<T, WRITER>;
pub type SetReader<T> = Set<T, READER>;

/// Replicated, unordered, eventually consistent set.
pub struct Set<T: Key, const IS_WRITER: bool = WRITER> {
	when: When,
	group: Group<SetStateMachine<T>>,
	data: watch::Receiver<HashSet<T>>,
}

// read-only access, available to both writers and readers
impl<T: Key, const IS_WRITER: bool> Set<T, IS_WRITER> {
	/// Get the number of elements in the set.
	///
	/// Time: O(1)
	pub fn len(&self) -> usize {
		self.data.borrow().len()
	}

	/// Test whether the set is empty.
	///
	/// Time: O(1)
	pub fn is_empty(&self) -> bool {
		self.data.borrow().is_empty()
	}

	/// Test whether the set contains a given value.
	///
	/// Time: O(log n)
	pub fn contains(&self, value: &T) -> bool {
		self.data.borrow().clone().contains(value)
	}

	/// Test whether this set is a subset of another set.
	///
	/// Time: O(n)
	pub fn is_subset<const W: bool>(&self, other: &Set<T, W>) -> bool {
		self
			.data
			.borrow()
			.clone()
			.is_subset(other.data.borrow().clone())
	}

	/// Get an iterator over the elements of the set.
	pub fn iter(&self) -> impl Iterator<Item = T> {
		let iter_clone = self.data.borrow().clone();
		iter_clone.into_iter()
	}

	/// Returns an observer of the set's state, which can be used to wait for
	/// the set to reach a certain state version before performing an action
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
impl<T: Key> SetWriter<T> {
	/// Discard all elements from the set.
	///
	/// This leaves you with an empty set, and all elements that
	/// were previously inside it are dropped.
	pub async fn clear(&self) -> Result<Version, Error<()>> {
		self
			.execute(SetCommand::Clear, |_| Error::Offline(()))
			.await
	}

	/// Insert a value into the set.
	///
	/// If the set already contained this value, the operation is a no-op.
	///
	/// Time: O(log n)
	pub async fn insert(&self, value: T) -> Result<Version, Error<T>> {
		self
			.execute(SetCommand::Insert { value }, |cmd| match cmd {
				SetCommand::Insert { value } => Error::Offline(value),
				_ => unreachable!(),
			})
			.await
	}

	/// Insert multiple values into the set.
	pub async fn extend(
		&self,
		values: impl IntoIterator<Item = T>,
	) -> Result<Version, Error<Vec<T>>> {
		let entries: Vec<T> = values.into_iter().collect();

		if entries.is_empty() {
			return Ok(Version(self.group.committed()));
		}

		self
			.execute(SetCommand::Extend { entries }, |cmd| match cmd {
				SetCommand::Extend { entries } => Error::Offline(entries),
				_ => unreachable!(),
			})
			.await
	}

	/// Remove a value from the set.
	///
	/// Time: O(log n)
	pub async fn remove(&self, value: &T) -> Result<Version, Error<T>> {
		let value = value.clone();
		self
			.execute(SetCommand::Remove { value }, |cmd| match cmd {
				SetCommand::Remove { value } => Error::Offline(value),
				_ => unreachable!(),
			})
			.await
	}
}

// construction
impl<T: Key, const IS_WRITER: bool> Set<T, IS_WRITER> {
	/// Create a new set in writer mode.
	///
	/// The returned writer can be used to modify the set, and it also provides
	/// read access to the set's contents. Writers can be used by multiple
	/// nodes concurrently, and all changes made by any writer will be replicated
	/// to all other writers and readers.
	///
	/// This creates a new set with default synchronization configuration. If you
	/// want to customize the synchronization behavior (e.g. snapshot sync
	/// configuration), use `writer_with_config` instead.
	///
	/// Note that different sync configurations will create different group ids
	/// and the resulting sets will not be able to see each other.
	pub fn writer(network: &Network, store_id: StoreId) -> SetWriter<T> {
		Self::writer_with_config(network, store_id, SyncConfig::default())
	}

	/// Create a new set in writer mode.
	///
	/// The returned writer can be used to modify the set, and it also provides
	/// read access to the set's contents. Writers can be used by multiple
	/// nodes concurrently, and all changes made by any writer will be replicated
	/// to all other writers and readers.
	///
	/// This creates a new set with the specified sync configuration. If you
	/// want to use the default sync configuration, use the `writer` method
	/// instead.
	///
	/// Note that different sync configurations will create different group ids
	/// and the resulting sets will not be able to see each other.
	pub fn writer_with_config(
		network: &Network,
		store_id: StoreId,
		config: SyncConfig,
	) -> SetWriter<T> {
		Self::create::<WRITER>(network, store_id, config)
	}

	/// Create a new set in writer mode.
	///
	/// This creates a new set with default synchronization configuration. If you
	/// want to customize the synchronization behavior (e.g. snapshot sync
	/// configuration), use `new_with_config` instead.
	///
	/// Note that different sync configurations will create different group ids
	/// and the resulting sets will not be able to see each other.
	pub fn new(network: &Network, store_id: StoreId) -> SetWriter<T> {
		Self::writer(network, store_id)
	}

	/// Create a new set in writer mode.
	///
	/// This creates a new set with the specified sync configuration. If you
	/// want to use the default sync configuration, use the `new` method
	/// instead.
	///
	/// Note that different sync configurations will create different group ids
	/// and the resulting sets will not be able to see each other.
	pub fn new_with_config(
		network: &Network,
		store_id: StoreId,
		config: SyncConfig,
	) -> SetWriter<T> {
		Self::writer_with_config(network, store_id, config)
	}

	/// Create a new set in reader mode.
	///
	/// The returned reader provides read-only access to the set's contents.
	/// Readers can be used by multiple nodes concurrently, and they will see all
	/// changes made by any writer. However, readers cannot modify the set, and
	/// they will not be able to make any changes themselves. Readers have longer
	/// election timeouts to reduce the likelihood of them being elected as
	/// group leaders, which reduces latency for read operations.
	pub fn reader(network: &Network, store_id: StoreId) -> SetReader<T> {
		Self::reader_with_config(network, store_id, SyncConfig::default())
	}

	/// Create a new set in reader mode.
	///
	/// The returned reader provides read-only access to the set's contents.
	/// Readers can be used by multiple nodes concurrently, and they will see all
	/// changes made by any writer. However, readers cannot modify the set, and
	/// they will not be able to make any changes themselves. Readers have longer
	/// election timeouts to reduce the likelihood of them being elected as
	/// group leaders, which reduces latency for read operations.
	pub fn reader_with_config(
		network: &Network,
		store_id: StoreId,
		config: SyncConfig,
	) -> SetReader<T> {
		Self::create::<READER>(network, store_id, config)
	}

	fn create<const W: bool>(
		network: &Network,
		store_id: StoreId,
		config: SyncConfig,
	) -> Set<T, W> {
		let machine = SetStateMachine::new(
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

		Set::<T, W> {
			when: When::new(group.when().clone()),
			group,
			data,
		}
	}
}

// internal
impl<T: Key> SetWriter<T> {
	async fn execute<TErr>(
		&self,
		command: SetCommand<T>,
		offline_err: impl FnOnce(SetCommand<T>) -> Error<TErr>,
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

struct SetStateMachine<T: Key> {
	data: HashSet<T>,
	latest: watch::Sender<HashSet<T>>,
	store_id: StoreId,
	local_id: PeerId,
	state_sync: SnapshotSync<Self>,
	is_writer: bool,
}

impl<T: Key> SetStateMachine<T> {
	pub fn new(
		store_id: StoreId,
		is_writer: bool,
		sync_config: SyncConfig,
		local_id: PeerId,
	) -> Self {
		let data = HashSet::default();
		let state_sync = SnapshotSync::new(sync_config, |request| {
			SetCommand::TakeSnapshot(request)
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

	pub fn data(&self) -> watch::Receiver<HashSet<T>> {
		self.latest.subscribe()
	}
}

impl<T: Key> StateMachine for SetStateMachine<T> {
	type Command = SetCommand<T>;
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
				SetCommand::Clear => {
					self.data.clear();
				}
				SetCommand::Insert { value } => {
					self.data.insert(value);
				}
				SetCommand::Remove { value } => {
					self.data.remove(&value);
				}
				SetCommand::Extend { entries } => {
					for value in entries {
						self.data.insert(value);
					}
				}
				SetCommand::TakeSnapshot(request) => {
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

	/// The group-key for a set is derived from the store ID and the type of
	/// the set's elements. This ensures that different sets (with different
	/// store IDs or element types) will be in different groups.
	fn signature(&self) -> crate::UniqueId {
		UniqueId::from("mosaik_collections_set")
			.derive(self.store_id)
			.derive(type_name::<T>())
	}

	/// This state machine doesn't support external queries, because all of its
	/// state is observable through the `latest` watch channel. Therefore, the
	/// query method is a no-op.
	fn query(&self, (): Self::Query) {}

	/// This state machine uses the `SnapshotSync` state sync strategy.
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

impl<T: Key> SnapshotStateMachine for SetStateMachine<T> {
	type Snapshot = SetSnapshot<T>;

	fn create_snapshot(&self) -> Self::Snapshot {
		SetSnapshot {
			data: self.data.clone(),
		}
	}

	fn install_snapshot(&mut self, snapshot: Self::Snapshot) {
		self.data = snapshot.data;
		self.latest.send_replace(self.data.clone());
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "T: Key")]
enum SetCommand<T> {
	Clear,
	Insert { value: T },
	Remove { value: T },
	Extend { entries: Vec<T> },
	TakeSnapshot(SnapshotRequest),
}

#[derive(Debug, Clone)]
pub struct SetSnapshot<T: Key> {
	data: HashSet<T>,
}

impl<T: Key> Default for SetSnapshot<T> {
	fn default() -> Self {
		Self {
			data: HashSet::default(),
		}
	}
}

impl<T: Key> Snapshot for SetSnapshot<T> {
	type Item = T;

	fn len(&self) -> u64 {
		self.data.len() as u64
	}

	fn iter_range(
		&self,
		range: Range<u64>,
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
				.cloned(),
		)
	}

	fn append(&mut self, items: impl IntoIterator<Item = Self::Item>) {
		self.data.extend(items);
	}
}
