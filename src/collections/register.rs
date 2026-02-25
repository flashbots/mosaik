use {
	super::{
		Error,
		READER,
		SyncConfig,
		WRITER,
		When,
		primitives::{StoreId, Value, Version},
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
	core::{
		any::type_name,
		ops::{Deref, Range},
	},
	serde::{Deserialize, Serialize},
	tokio::sync::watch,
};

/// Mutable access to a replicated register.
///
/// Has higher priority for assuming group leadership.
pub type RegisterWriter<T> = Register<T, WRITER>;

/// Read-only access to a register.
///
/// Has lower priority for assuming group leadership.
pub type RegisterReader<T> = Register<T, READER>;

/// Replicated single-value register.
///
/// A register holds at most one value at a time. Writing a new value
/// replaces the previous one. This is the distributed equivalent of a
/// `tokio::sync::watch` channel — all nodes observe the latest value.
pub struct Register<T: Value, const IS_WRITER: bool = WRITER> {
	when: When,
	group: Group<RegisterStateMachine<T>>,
	data: watch::Receiver<Option<T>>,
}

// read-only access, available to both readers and writers
impl<T: Value, const IS_WRITER: bool> Register<T, IS_WRITER> {
	/// Read the current value of the register.
	///
	/// Returns `None` if no value has been written yet.
	///
	/// Time: O(1)
	pub fn read(&self) -> Option<T> {
		self.data.borrow().clone()
	}

	/// Read the current value of the register.
	///
	/// Returns `None` if no value has been written yet.
	///
	/// Time: O(1)
	pub fn get(&self) -> Option<T> {
		self.read()
	}

	/// Test whether the register contains a value.
	///
	/// Time: O(1)
	pub fn is_empty(&self) -> bool {
		self.data.borrow().is_none()
	}

	/// Test whether the register contains a value.
	///
	/// Time: O(1)
	pub fn is_none(&self) -> bool {
		self.is_empty()
	}

	/// Test whether the register contains a value.
	///
	/// Time: O(1)
	pub fn is_some(&self) -> bool {
		!self.is_empty()
	}

	/// Returns an observer of the register's state, which can be used to wait
	/// for the register to reach a certain state version before performing an
	/// action or knowing when it is online or offline.
	pub const fn when(&self) -> &When {
		&self.when
	}

	/// The current version of the register's state, which is the version of the
	/// latest committed state.
	pub fn version(&self) -> Version {
		Version(self.group.committed())
	}
}

// Mutable operations, only available to writers
impl<T: Value> RegisterWriter<T> {
	/// Write a new value to the register, replacing the previous one.
	///
	/// Time: O(1)
	pub async fn write(&self, value: T) -> Result<Version, Error<T>> {
		self
			.execute(RegisterCommand::Write { value }, |cmd| match cmd {
				RegisterCommand::Write { value } => Error::Offline(value),
				_ => unreachable!(),
			})
			.await
	}

	/// Write a new value to the register, replacing the previous one.
	///
	/// Time: O(1)
	pub async fn set(&self, value: T) -> Result<Version, Error<T>> {
		self.write(value).await
	}

	/// Compare the current value of the register with an expected value, and if
	/// they match, replace it with a new value. If the current value does not
	/// match the expected value, no write occurs.
	///
	/// Time: O(1)
	pub async fn compare_exchange(
		&self,
		current: Option<T>,
		new: Option<T>,
	) -> Result<Version, Error<(Option<T>, Option<T>)>> {
		self
			.execute(RegisterCommand::CompareExchange { current, new }, |cmd| {
				match cmd {
					RegisterCommand::CompareExchange { current, new } => {
						Error::Offline((current, new))
					}
					_ => unreachable!(),
				}
			})
			.await
	}

	/// Clear the register, removing the stored value.
	///
	/// After this operation, `read()` will return `None`.
	///
	/// Time: O(1)
	pub async fn clear(&self) -> Result<Version, Error<()>> {
		self
			.execute(RegisterCommand::Clear, |_| Error::Offline(()))
			.await
	}
}

// construction
impl<T: Value, const IS_WRITER: bool> Register<T, IS_WRITER> {
	/// Create a new register in writer mode.
	///
	/// The returned writer can be used to modify the register, and it also
	/// provides read access to the register's contents. Writers can be used by
	/// multiple nodes concurrently, and all changes made by any writer will be
	/// replicated to all other writers and readers.
	///
	/// This creates a new register with default synchronization configuration.
	/// If you want to customize the synchronization behavior, use
	/// `writer_with_config` instead.
	///
	/// Note that different sync configurations will create different group ids
	/// and the resulting registers will not be able to see each other.
	pub fn writer(network: &Network, store_id: StoreId) -> RegisterWriter<T> {
		Self::writer_with_config(network, store_id, SyncConfig::default())
	}

	/// Create a new register in writer mode.
	///
	/// The returned writer can be used to modify the register, and it also
	/// provides read access to the register's contents. Writers can be used by
	/// multiple nodes concurrently, and all changes made by any writer will be
	/// replicated to all other writers and readers.
	///
	/// This creates a new register with the specified sync configuration. If you
	/// want to use the default sync configuration, use the `writer` method
	/// instead.
	///
	/// Note that different sync configurations will create different group ids
	/// and the resulting registers will not be able to see each other.
	pub fn writer_with_config(
		network: &Network,
		store_id: StoreId,
		config: SyncConfig,
	) -> RegisterWriter<T> {
		Self::create::<WRITER>(network, store_id, config)
	}

	/// Create a new register in writer mode.
	///
	/// This is an alias for the `writer` method.
	pub fn new(network: &Network, store_id: StoreId) -> RegisterWriter<T> {
		Self::writer(network, store_id)
	}

	/// Create a new register in writer mode with the specified sync
	/// configuration.
	///
	/// This is an alias for the `writer_with_config` method.
	pub fn new_with_config(
		network: &Network,
		store_id: StoreId,
		config: SyncConfig,
	) -> RegisterWriter<T> {
		Self::writer_with_config(network, store_id, config)
	}

	/// Create a new register in reader mode.
	///
	/// The returned reader provides read-only access to the register's contents.
	/// Readers can be used by multiple nodes concurrently, and they will see all
	/// changes made by any writer. However, readers cannot modify the register,
	/// and they will not be able to make any changes themselves. Readers have
	/// longer election timeouts to reduce the likelihood of them being elected
	/// as group leaders, which reduces latency for read operations.
	///
	/// This creates a new register with the default sync configuration. If you
	/// want to specify a custom sync configuration, use the
	/// `reader_with_config` method instead.
	pub fn reader(network: &Network, store_id: StoreId) -> RegisterReader<T> {
		Self::reader_with_config(network, store_id, SyncConfig::default())
	}

	/// Create a new register in reader mode.
	///
	/// The returned reader provides read-only access to the register's contents.
	/// Readers can be used by multiple nodes concurrently, and they will see all
	/// changes made by any writer. However, readers cannot modify the register,
	/// and they will not be able to make any changes themselves. Readers have
	/// longer election timeouts to reduce the likelihood of them being elected
	/// as group leaders, which reduces latency for read operations.
	///
	/// This creates a new register with the specified sync configuration. If you
	/// want to use the default sync configuration, use the `reader` method
	/// instead.
	///
	/// Note that different sync configurations will create different group ids
	/// and the resulting registers will not be able to see each other.
	pub fn reader_with_config(
		network: &Network,
		store_id: StoreId,
		config: SyncConfig,
	) -> RegisterReader<T> {
		Self::create::<READER>(network, store_id, config)
	}

	fn create<const W: bool>(
		network: &Network,
		store_id: StoreId,
		config: SyncConfig,
	) -> Register<T, W> {
		let machine = RegisterStateMachine::new(
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
		let when = When::new(group.when().clone());

		Register::<T, W> { when, group, data }
	}
}

// internal
impl<T: Value> RegisterWriter<T> {
	async fn execute<TErr>(
		&self,
		command: RegisterCommand<T>,
		offline_err: impl FnOnce(RegisterCommand<T>) -> Error<TErr>,
	) -> Result<Version, Error<TErr>> {
		self
			.group
			.execute(command)
			.await
			.map(Version)
			.map_err(|err| match err {
				CommandError::Offline(mut items) => offline_err(items.remove(0)),
				CommandError::GroupTerminated => Error::NetworkDown,
				CommandError::NoCommands => unreachable!(),
			})
	}
}

struct RegisterStateMachine<T: Value> {
	data: Option<T>,
	latest: watch::Sender<Option<T>>,
	store_id: StoreId,
	local_id: PeerId,
	state_sync: SnapshotSync<Self>,
	is_writer: bool,
}

impl<T: Value> RegisterStateMachine<T> {
	pub fn new(
		store_id: StoreId,
		is_writer: bool,
		sync_config: SyncConfig,
		local_id: PeerId,
	) -> Self {
		let data = None;
		let state_sync = SnapshotSync::new(sync_config, |request| {
			RegisterCommand::TakeSnapshot(request)
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

	pub fn data(&self) -> watch::Receiver<Option<T>> {
		self.latest.subscribe()
	}
}

impl<T: Value> StateMachine for RegisterStateMachine<T> {
	type Command = RegisterCommand<T>;
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
				RegisterCommand::Write { value } => {
					self.data = Some(value);
				}
				RegisterCommand::CompareExchange { current, new } => {
					if self.data == current {
						self.data = new;
					}
				}
				RegisterCommand::Clear => {
					self.data = None;
				}
				RegisterCommand::TakeSnapshot(request) => {
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

	/// The group-key for a register is derived from the store ID and the type
	/// of the register's value. This ensures that different registers (with
	/// different store IDs or value types) will be in different groups.
	fn signature(&self) -> crate::UniqueId {
		UniqueId::from("mosaik_collections_register")
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

impl<T: Value> SnapshotStateMachine for RegisterStateMachine<T> {
	type Snapshot = RegisterSnapshot<T>;

	fn create_snapshot(&self) -> Self::Snapshot {
		RegisterSnapshot {
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
enum RegisterCommand<T> {
	Write { value: T },
	CompareExchange { current: Option<T>, new: Option<T> },
	Clear,
	TakeSnapshot(SnapshotRequest),
}

#[derive(Debug, Clone)]
pub struct RegisterSnapshot<T: Value> {
	data: Option<T>,
}

impl<T: Value> Default for RegisterSnapshot<T> {
	fn default() -> Self {
		Self { data: None }
	}
}

impl<T: Value> Snapshot for RegisterSnapshot<T> {
	type Item = T;

	fn len(&self) -> u64 {
		u64::from(self.data.is_some())
	}

	fn iter_range(
		&self,
		range: Range<u64>,
	) -> Option<impl Iterator<Item = Self::Item>> {
		if range.contains(&0) {
			Some(self.data.clone().into_iter())
		} else {
			None
		}
	}

	fn append(&mut self, items: impl IntoIterator<Item = Self::Item>) {
		// For a register, the last item wins
		for item in items {
			self.data = Some(item);
		}
	}
}
