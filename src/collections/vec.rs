use {
	super::{
		Error,
		READER,
		SyncConfig,
		WRITER,
		When,
		primitives::{Key, StoreId, Value, Version},
		sync::{Snapshot, SnapshotStateMachine, SnapshotSync},
	},
	crate::{
		Group,
		Network,
		PeerId,
		UniqueId,
		collections::sync::{SnapshotInfo, SnapshotRequest},
		groups::{
			ApplyContext,
			CommandError,
			ConsensusConfig,
			Cursor,
			Index,
			IndexRange,
			StateMachine,
			SyncContext,
			Term,
		},
		primitives::{Short, UnboundedChannel},
	},
	chrono::{DateTime, Utc},
	core::{any::type_name, cmp::Ordering, ops::Range},
	serde::{Deserialize, Serialize},
	tokio::sync::{broadcast, mpsc::UnboundedSender, watch},
};

pub type VecWriter<T> = Vec<T, WRITER>;
pub type VecReader<T> = Vec<T, READER>;

/// Replicated ordered, index-addressable sequence.
pub struct Vec<T: Value, const IS_WRITER: bool = WRITER> {
	when: When,
	group: Group<VecStateMachine<T>>,
	data: watch::Receiver<im::Vector<T>>,
}

// read-only access, available to both readers and writers
impl<T: Value, const IS_WRITER: bool> Vec<T, IS_WRITER> {
	/// Get the length of a vector.
	///
	/// Time: O(1)
	pub fn len(&self) -> usize {
		self.data.borrow().len()
	}

	/// Test whether a vector is empty.
	///
	/// Time: O(1)
	pub fn is_empty(&self) -> bool {
		self.data.borrow().is_empty()
	}

	/// Get the last element of a vector.
	///
	/// If the vector is empty, `None` is returned.
	///
	/// Time: O(log n)
	pub fn back(&self) -> Option<T> {
		self.data.borrow().back().cloned()
	}

	/// Get the last element of a vector.
	///
	/// If the vector is empty, `None` is returned.
	///
	/// This is an alias for the [`back`][back] method.
	///
	/// Time: O(log n)
	pub fn last(&self) -> Option<T> {
		self.data.borrow().last().cloned()
	}

	/// Get the first element of a vector.
	///
	/// If the vector is empty, `None` is returned.
	///
	/// Time: O(log n)
	pub fn front(&self) -> Option<T> {
		self.data.borrow().front().cloned()
	}

	/// Get the first element of a vector.
	///
	/// If the vector is empty, `None` is returned.
	///
	/// This is an alias for the [`front`][front] method.
	///
	/// Time: O(log n)
	pub fn head(&self) -> Option<T> {
		self.data.borrow().head().cloned()
	}

	/// Test if a given element is in the vector.
	///
	/// Searches the vector for the first occurrence of a given value,
	/// and returns `true` if it's there. If it's nowhere to be found
	/// in the vector, it returns `false`.
	///
	/// Time: O(n)
	pub fn contains(&self, value: &T) -> bool {
		self.data.borrow().clone().contains(value)
	}

	/// Get a clone of the value at index `index` in a vector.
	///
	/// Returns `None` if the index is out of bounds.
	///
	/// Time: O(log n)
	pub fn get(&self, index: u64) -> Option<T> {
		self.data.borrow().get(index as usize).cloned()
	}

	/// Get the index of a given element in the vector.
	///
	/// Searches the vector for the first occurrence of a given value,
	/// and returns the index of the value if it's there. Otherwise,
	/// it returns `None`.
	///
	/// Time: O(n)
	pub fn index_of(&self, value: &T) -> Option<u64> {
		self.data.borrow().clone().index_of(value).map(|i| i as u64)
	}

	/// Get an iterator over a vector.
	pub fn iter(&self) -> impl Iterator<Item = T> {
		let iter_clone = self.data.borrow().clone();
		iter_clone.into_iter()
	}

	/// Returns an observer of the vector's state, which can be used to wait for
	/// the vector to reach a certain state version before performing an action
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
impl<T: Value> VecWriter<T> {
	/// Discard all elements from the vector.
	///
	/// This leaves you with an empty vector, and all elements that
	/// were previously inside it are dropped.
	pub async fn clear(&self) -> Result<Version, Error<()>> {
		self
			.execute(VecCommand::Clear, |_| Error::Offline(()))
			.await
	}

	/// Push a value to the back of a vector.
	///
	/// Time: O(1)*
	pub async fn push_back(&self, value: T) -> Result<Version, Error<T>> {
		self
			.execute(VecCommand::PushBack { value }, |cmd| match cmd {
				VecCommand::PushBack { value } => Error::Offline(value),
				_ => unreachable!(),
			})
			.await
	}

	/// Push a value to the front of a vector.
	///
	/// Time: O(1)*
	pub async fn push_front(&self, value: T) -> Result<Version, Error<T>> {
		self
			.execute(VecCommand::PushFront { value }, |cmd| match cmd {
				VecCommand::PushFront { value } => Error::Offline(value),
				_ => unreachable!(),
			})
			.await
	}

	/// Swap the elements at indices `i` and `j`.
	pub async fn swap(&self, i: u64, j: u64) -> Result<Version, Error<()>> {
		self
			.execute(VecCommand::Swap { i, j }, |_| Error::Offline(()))
			.await
	}

	/// Insert an element into a vector.
	///
	/// Insert an element at position `index`, shifting all elements
	/// after it to the right.
	pub async fn insert(
		&self,
		index: u64,
		value: T,
	) -> Result<Version, Error<T>> {
		self
			.execute(VecCommand::Insert { index, value }, |cmd| match cmd {
				VecCommand::Insert { value, .. } => Error::Offline(value),
				_ => unreachable!(),
			})
			.await
	}

	/// Append multiple values to the back of a vector.
	pub async fn extend(
		&self,
		entries: impl IntoIterator<Item = T>,
	) -> Result<Version, Error<std::vec::Vec<T>>> {
		let entries: std::vec::Vec<T> = entries.into_iter().collect();

		if entries.is_empty() {
			return Ok(Version(self.group.committed()));
		}

		self
			.execute(VecCommand::Extend { entries }, |cmd| match cmd {
				VecCommand::Extend { entries } => Error::Offline(entries),
				_ => unreachable!(),
			})
			.await
	}

	/// Remove the last element from a vector and return it.
	///
	/// Time: O(1)*
	pub async fn pop_back(&self) -> Result<Version, Error<()>> {
		self
			.execute(VecCommand::PopBack, |_| Error::Offline(()))
			.await
	}

	/// Remove the first element from a vector and return it.
	///
	/// Time: O(1)*
	pub async fn pop_front(&self) -> Result<Version, Error<()>> {
		self
			.execute(VecCommand::PopFront, |_| Error::Offline(()))
			.await
	}

	/// Remove an element from a vector.
	///
	/// Remove the element from position 'index', shifting all
	/// elements after it to the left, and return the removed element.
	pub async fn remove(&self, index: u64) -> Result<Version, Error<u64>> {
		self
			.execute(VecCommand::Remove { index }, |cmd| match cmd {
				VecCommand::Remove { index } => Error::Offline(index),
				_ => unreachable!(),
			})
			.await
	}

	/// Truncate a vector to the given size.
	///
	/// Discards all elements in the vector beyond the given length.
	///
	/// Time: O(log n)
	pub async fn truncate(&self, len: usize) -> Result<Version, Error<()>> {
		let len = len as u64;
		self
			.execute(VecCommand::Truncate { len }, |_| Error::Offline(()))
			.await
	}
}

// construction
impl<T: Value, const IS_WRITER: bool> Vec<T, IS_WRITER> {
	/// Create a new vector in writer mode.
	///
	/// The returned writer can be used to modify the vector, and it also provides
	/// read access to the vector's contents. Writers can be used by multiple
	/// nodes concurrently, and all changes made by any writer will be replicated
	/// to all other writers and readers.
	///
	/// This create a new vector with the default sync configuration. If you want
	/// to specify a custom sync configuration, use the `writer_with_sync_config`
	/// method instead.
	///
	/// Note that different sync configurations will create different groups ids
	/// and the resulting vectors will not be able to see each other.
	pub fn writer(
		network: &crate::network::Network,
		store_id: crate::UniqueId,
	) -> VecWriter<T> {
		Self::writer_with_sync_config(network, store_id, SyncConfig::default())
	}

	/// Create a new vector in writer mode.
	///
	/// The returned writer can be used to modify the vector, and it also provides
	/// read access to the vector's contents. Writers can be used by multiple
	/// nodes concurrently, and all changes made by any writer will be replicated
	/// to all other writers and readers.
	///
	/// This create a new vector with the specified sync configuration. If you
	/// want to use the default sync configuration, use the `writer` method
	/// instead.
	///
	/// Note that different sync configurations will create different groups ids
	/// and the resulting vectors will not be able to see each other.
	pub fn writer_with_sync_config(
		network: &crate::network::Network,
		store_id: crate::UniqueId,
		sync_config: SyncConfig,
	) -> VecWriter<T> {
		let machine = VecStateMachine::new(
			store_id, //
			WRITER,
			sync_config,
			network.local().id(),
		);

		let data = machine.data();
		let group = network
			.groups()
			.with_key(store_id.into())
			.with_state_machine(machine)
			.join();
		let when = When::new(group.when().clone());

		VecWriter::<T> { when, group, data }
	}

	/// Create a new vector in reader mode.
	///
	/// The returned reader provides read-only access to the vector's contents.
	/// Readers can be used by multiple nodes concurrently, and they will see all
	/// changes made by any writer. However, readers cannot modify the vector, and
	/// they will not be able to make any changes themselves. Readers have longer
	/// election timeouts to reduce the likelihood of them being elected as
	/// group leaders, which reduces latency for read operations.
	///
	/// This creates a new vector with the specified sync configuration. If you
	/// want to use the default sync configuration, use the `reader` method
	/// instead.
	///
	/// Note that different sync configurations will create different groups ids
	/// and the resulting vectors will not be able to see each other. Make sure
	/// that the sync configuration used for readers is compatible with the sync
	/// configuration used for writers, otherwise the readers will not see any of
	/// the writers' changes.
	pub fn reader_with_sync_config(
		network: &Network,
		store_id: StoreId,
		sync_config: SyncConfig,
	) -> VecReader<T> {
		let machine = VecStateMachine::new(
			store_id, //
			READER,
			sync_config,
			network.local().id(),
		);

		let data = machine.data();
		let group = network
			.groups()
			.with_key(store_id.into())
			.with_state_machine(machine)
			.join();
		let when = When::new(group.when().clone());

		VecReader::<T> { when, group, data }
	}

	/// Create a new vector in writer mode.
	///
	/// This is an alias for the `writer` method.
	pub fn new(network: &Network, store_id: StoreId) -> VecWriter<T> {
		VecWriter::<T>::writer(network, store_id)
	}

	/// Create a new vector in writer mode with the specified sync configuration.
	///
	/// This is an alias for the `writer_with_sync_config` method.
	pub fn new_with_sync_config(
		network: &Network,
		store_id: StoreId,
		sync_config: SyncConfig,
	) -> VecWriter<T> {
		VecWriter::<T>::writer_with_sync_config(network, store_id, sync_config)
	}

	/// Create a new vector in reader mode.
	///
	/// The returned reader provides read-only access to the vector's contents.
	/// Readers can be used by multiple nodes concurrently, and they will see all
	/// changes made by any writer. However, readers cannot modify the vector, and
	/// they will not be able to make any changes themselves. Readers have longer
	/// election timeouts to reduce the likelihood of them being elected as
	/// group leaders, which reduces latency for read operations.
	///
	/// This creates a new vector with the default sync configuration. If you want
	/// to specify a custom sync configuration, use the `reader_with_sync_config`
	/// method instead.
	pub fn reader(network: &Network, store_id: StoreId) -> VecReader<T> {
		Self::reader_with_sync_config(network, store_id, SyncConfig::default())
	}
}

// internal
impl<T: Value> Vec<T, WRITER> {
	async fn execute<TErr>(
		&self,
		command: VecCommand<T>,
		offline_err: impl FnOnce(VecCommand<T>) -> Error<TErr>,
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

struct VecStateMachine<T: Value> {
	data: im::Vector<T>,
	latest: watch::Sender<im::Vector<T>>,
	store_id: StoreId,
	is_writer: bool,
	state_sync: SnapshotSync<Self>,
	local_id: PeerId,
}

impl<T: Value> VecStateMachine<T> {
	pub fn new(
		store_id: StoreId,
		is_writer: bool,
		sync_config: SyncConfig,
		local_id: PeerId,
	) -> Self {
		let data = im::Vector::new();
		let state_sync = SnapshotSync::new(sync_config, |request| {
			VecCommand::TakeSnapshot(request)
		});
		let latest = watch::Sender::new(data.clone());

		Self {
			data,
			latest,
			store_id,
			is_writer,
			state_sync,
			local_id,
		}
	}

	pub fn data(&self) -> watch::Receiver<im::Vector<T>> {
		self.latest.subscribe()
	}
}

impl<T: Value> StateMachine for VecStateMachine<T> {
	type Command = VecCommand<T>;
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
				VecCommand::Clear => {
					self.data.clear();
				}
				VecCommand::Swap { i, j } => {
					if i < self.data.len() as u64 && j < self.data.len() as u64 {
						self.data.swap(i as usize, j as usize);
					}
				}
				VecCommand::Insert { index, value } => {
					self.data.insert(index as usize, value);
				}
				VecCommand::PushBack { value } => {
					self.data.push_back(value);
				}
				VecCommand::PushFront { value } => {
					self.data.push_front(value);
				}
				VecCommand::PopBack => {
					self.data.pop_back();
				}
				VecCommand::PopFront => {
					self.data.pop_front();
				}
				VecCommand::Remove { index } => {
					if index < self.data.len() as u64 {
						self.data.remove(index as usize);
					}
				}
				VecCommand::Truncate { len } => {
					let len = len as usize;
					let len = len.min(self.data.len());
					self.data.truncate(len);
				}
				VecCommand::Extend { entries } => {
					self.data.extend(entries);
				}
				VecCommand::TakeSnapshot(request) => {
					if request.requested_by != self.local_id
						&& !self.state_sync.is_expired(&request)
					{
						// take note of the snapshot request, we will take the actual
						// snapshot after applying all commands from this batch.
						tracing::info!(
							"..--> Received snapshot request from {}, local_id = {}",
							Short(request.requested_by),
							Short(self.local_id)
						);
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

	/// The group-key for a vector is derived from the store ID and the type of
	/// the vector's elements. This ensures that different vectors (with different
	/// store IDs or element types) will be in different groups.
	fn signature(&self) -> crate::UniqueId {
		UniqueId::from("mosaik_collections_vec")
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

impl<T: Value> SnapshotStateMachine for VecStateMachine<T> {
	type Snapshot = VecSnapshot<T>;

	fn create_snapshot(&self) -> Self::Snapshot {
		VecSnapshot {
			data: self.data.clone(),
		}
	}

	fn install_snapshot(&mut self, snapshot: Self::Snapshot) {
		self.data = snapshot.data;
		self.latest.send_replace(self.data.clone());
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "T: Value")]
enum VecCommand<T> {
	Clear,
	Swap { i: u64, j: u64 },
	Insert { index: u64, value: T },
	PushBack { value: T },
	PushFront { value: T },
	PopBack,
	PopFront,
	Remove { index: u64 },
	Truncate { len: u64 },
	Extend { entries: std::vec::Vec<T> },
	TakeSnapshot(SnapshotRequest),
}

#[derive(Debug, Clone)]
pub struct VecSnapshot<T: Value> {
	data: im::Vector<T>,
}

impl<T: Value> Snapshot for VecSnapshot<T> {
	type Item = T;

	fn len(&self) -> u64 {
		self.data.len() as u64
	}

	fn iter_range(
		&self,
		range: Range<u64>,
	) -> Option<impl Iterator<Item = Self::Item>> {
		let skip = range.start as usize;
		let take = (range.end - range.start) as usize;

		if skip + take > self.data.len() {
			return None;
		}

		Some(self.data.skip(skip).take(take).into_iter())
	}

	fn append(&mut self, items: impl IntoIterator<Item = Self::Item>) {
		self.data.extend(items);
	}
}

impl<T: Value> Default for VecSnapshot<T> {
	fn default() -> Self {
		Self {
			data: im::Vector::new(),
		}
	}
}
