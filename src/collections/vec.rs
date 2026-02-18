use {
	super::*,
	crate::{
		Group,
		Network,
		UniqueId,
		groups::{CommandError, ConsensusConfig, LogReplaySync, StateMachine},
	},
	core::{any::type_name, cmp::Ordering},
	serde::{Deserialize, Serialize},
	tokio::sync::watch,
};

pub type VecWriter<T> = Vec<T, WRITER>;
pub type VecReader<T> = Vec<T, READER>;

/// Replicated ordered, index-addressable sequence.
pub struct Vec<T: Value, const IS_WRITER: bool = WRITER> {
	when: When,
	group: Group<VecStateMachine<T>>,
	snapshot: watch::Receiver<im::Vector<T>>,
}

// read-only access, available to both readers and writers
impl<T: Value, const IS_WRITER: bool> Vec<T, IS_WRITER> {
	/// Get the length of a vector.
	///
	/// Time: O(1)
	pub fn len(&self) -> usize {
		self.snapshot.borrow().len()
	}

	/// Test whether a vector is empty.
	///
	/// Time: O(1)
	pub fn is_empty(&self) -> bool {
		self.snapshot.borrow().is_empty()
	}

	/// Get the last element of a vector.
	///
	/// If the vector is empty, `None` is returned.
	///
	/// Time: O(log n)
	pub fn back(&self) -> Option<T> {
		self.snapshot.borrow().back().cloned()
	}

	/// Get the last element of a vector.
	///
	/// If the vector is empty, `None` is returned.
	///
	/// This is an alias for the [`back`][back] method.
	///
	/// Time: O(log n)
	pub fn last(&self) -> Option<T> {
		self.snapshot.borrow().last().cloned()
	}

	/// Get the first element of a vector.
	///
	/// If the vector is empty, `None` is returned.
	///
	/// Time: O(log n)
	pub fn front(&self) -> Option<T> {
		self.snapshot.borrow().front().cloned()
	}

	/// Get the first element of a vector.
	///
	/// If the vector is empty, `None` is returned.
	///
	/// This is an alias for the [`front`][front] method.
	///
	/// Time: O(log n)
	pub fn head(&self) -> Option<T> {
		self.snapshot.borrow().head().cloned()
	}

	/// Test if a given element is in the vector.
	///
	/// Searches the vector for the first occurrence of a given value,
	/// and returns `true` if it's there. If it's nowhere to be found
	/// in the vector, it returns `false`.
	///
	/// Time: O(n)
	pub fn contains(&self, value: &T) -> bool {
		self.snapshot.borrow().clone().contains(value)
	}

	/// Get a clone of the value at index `index` in a vector.
	///
	/// Returns `None` if the index is out of bounds.
	///
	/// Time: O(log n)
	pub fn get(&self, index: u64) -> Option<T> {
		self.snapshot.borrow().get(index as usize).cloned()
	}

	/// Get the index of a given element in the vector.
	///
	/// Searches the vector for the first occurrence of a given value,
	/// and returns the index of the value if it's there. Otherwise,
	/// it returns `None`.
	///
	/// Time: O(n)
	pub fn index_of(&self, value: &T) -> Option<u64> {
		self
			.snapshot
			.borrow()
			.clone()
			.index_of(value)
			.map(|i| i as u64)
	}

	/// Get an iterator over a vector.
	pub fn iter(&self) -> impl Iterator<Item = T> {
		let iter_clone = self.snapshot.borrow().clone();
		iter_clone.into_iter()
	}

	pub const fn when(&self) -> &When {
		&self.when
	}
}

impl<T: Value> Vec<T, WRITER> {
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
	pub fn writer(
		network: &crate::network::Network,
		store_id: crate::UniqueId,
	) -> VecWriter<T> {
		let (machine, snapshot) = VecStateMachine::new(store_id, true);
		let group = network
			.groups()
			.with_key(store_id.into())
			.with_state_machine(machine)
			.join();

		VecWriter::<T> {
			when: When::new(group.when().clone()),
			group,
			snapshot,
		}
	}

	/// Create a new vector in reader mode.
	///
	/// The returned reader provides read-only access to the vector's contents.
	/// Readers can be used by multiple nodes concurrently, and they will see all
	/// changes made by any writer. However, readers cannot modify the vector, and
	/// they will not be able to make any changes themselves. Readers have longer
	/// election timeouts to reduce the likelihood of them being elected as
	/// group leaders, which reduces latency for read operations.
	pub fn reader(network: &Network, store_id: StoreId) -> VecReader<T> {
		let (machine, snapshot) = VecStateMachine::new(store_id, false);
		let group = network
			.groups()
			.with_key(store_id.into())
			.with_state_machine(machine)
			.join();

		VecReader::<T> {
			when: When::new(group.when().clone()),
			group,
			snapshot,
		}
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
}

impl<T: Value> VecStateMachine<T> {
	pub fn new(
		store_id: StoreId,
		is_writer: bool,
	) -> (Self, watch::Receiver<im::Vector<T>>) {
		let data = im::Vector::new();
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

impl<T: Value> StateMachine for VecStateMachine<T> {
	type Command = VecCommand<T>;
	type Query = ();
	type QueryResult = ();
	type StateSync = LogReplaySync<Self>;

	fn apply(&mut self, command: Self::Command) {
		self.apply_batch([command]);
	}

	fn apply_batch(&mut self, commands: impl IntoIterator<Item = Self::Command>) {
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
			}
		}
		self.latest.send_replace(self.data.clone());
	}

	fn signature(&self) -> crate::UniqueId {
		UniqueId::from("mosaik_collections_vec")
			.derive(self.store_id)
			.derive(type_name::<T>())
	}

	/// This state machine doesn't support queries, so we ignore the query input
	/// and always return `()`. All data is exposed through the shared snapshot,
	/// so there's no need for queries.
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
}
