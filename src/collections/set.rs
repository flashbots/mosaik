use {
	super::{
		Error,
		READER,
		WRITER,
		When,
		primitives::{Key, StoreId, Version},
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

pub type SetWriter<T> = Set<T, WRITER>;
pub type SetReader<T> = Set<T, READER>;

/// Replicated, unordered, eventually consistent set.
pub struct Set<T: Key, const IS_WRITER: bool = WRITER> {
	when: When,
	group: Group<SetStateMachine<T>>,
	snapshot: watch::Receiver<im::HashSet<T>>,
}

// read-only access, available to both writers and readers
impl<T: Key, const IS_WRITER: bool> Set<T, IS_WRITER> {
	/// Get the number of elements in the set.
	///
	/// Time: O(1)
	pub fn len(&self) -> usize {
		self.snapshot.borrow().len()
	}

	/// Test whether the set is empty.
	///
	/// Time: O(1)
	pub fn is_empty(&self) -> bool {
		self.snapshot.borrow().is_empty()
	}

	/// Test whether the set contains a given value.
	///
	/// Time: O(log n)
	pub fn contains(&self, value: &T) -> bool {
		self.snapshot.borrow().contains(value)
	}

	/// Test whether this set is a subset of another set.
	///
	/// Time: O(n)
	pub fn is_subset<const W: bool>(&self, other: &Set<T, W>) -> bool {
		self
			.snapshot
			.borrow()
			.is_subset(other.snapshot.borrow().clone())
	}

	/// Get an iterator over the elements of the set.
	pub fn iter(&self) -> impl Iterator<Item = T> {
		let iter_clone = self.snapshot.borrow().clone();
		iter_clone.into_iter()
	}

	/// Returns an observer of the set's state, which can be used to wait for
	/// the set to reach a certain state version before performing an action
	/// or knowing when it is online or offline.
	pub const fn when(&self) -> &When {
		&self.when
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
	pub fn writer(network: &Network, store_id: StoreId) -> SetWriter<T> {
		let (machine, snapshot) = SetStateMachine::new(store_id, true);
		let group = network
			.groups()
			.with_key(store_id.into())
			.with_state_machine(machine)
			.join();

		SetWriter::<T> {
			when: When::new(group.when().clone()),
			group,
			snapshot,
		}
	}

	/// Create a new set in writer mode.
	pub fn new(network: &Network, store_id: StoreId) -> SetWriter<T> {
		SetWriter::<T>::writer(network, store_id)
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
		let (machine, snapshot) = SetStateMachine::new(store_id, false);
		let group = network
			.groups()
			.with_key(store_id.into())
			.with_state_machine(machine)
			.join();

		SetReader::<T> {
			when: When::new(group.when().clone()),
			group,
			snapshot,
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
	data: im::HashSet<T>,
	latest: watch::Sender<im::HashSet<T>>,
	store_id: StoreId,
	is_writer: bool,
}

impl<T: Key> SetStateMachine<T> {
	pub fn new(
		store_id: StoreId,
		is_writer: bool,
	) -> (Self, watch::Receiver<im::HashSet<T>>) {
		let data = im::HashSet::new();
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

impl<T: Key> StateMachine for SetStateMachine<T> {
	type Command = SetCommand<T>;
	type Query = ();
	type QueryResult = ();
	type StateSync = LogReplaySync<Self>;

	fn apply(&mut self, command: Self::Command) {
		self.apply_batch([command]);
	}

	fn apply_batch(&mut self, commands: impl IntoIterator<Item = Self::Command>) {
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
			}
		}
		self.latest.send_replace(self.data.clone());
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
#[serde(bound = "T: Key")]
enum SetCommand<T> {
	Clear,
	Insert { value: T },
	Remove { value: T },
	Extend { entries: Vec<T> },
}
