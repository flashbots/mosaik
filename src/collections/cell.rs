use {
	super::{
		CollectionConfig,
		CollectionFromDef,
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
			Cursor,
			LeadershipPreference,
			StateMachine,
		},
		primitives::{EncodeError, Encoded},
	},
	core::{
		any::type_name,
		ops::{Deref, Range},
	},
	futures::{FutureExt, TryFutureExt},
	serde::{Deserialize, Serialize},
	tokio::sync::watch,
};

/// Mutable access to a replicated cell.
///
/// Has higher priority for assuming group leadership.
pub type CellWriter<T> = Cell<T, WRITER>;

/// Read-only access to a cell.
///
/// Has lower priority for assuming group leadership.
pub type CellReader<T> = Cell<T, READER>;

/// Replicated single-value cell.
///
/// A cell holds at most one value at a time. Writing a new value
/// replaces the previous one. This is the distributed equivalent of a
/// `tokio::sync::watch` channel — all nodes observe the latest value.
pub struct Cell<T: Value, const IS_WRITER: bool = WRITER> {
	when: When,
	group: Group<CellStateMachine<T>>,
	data: watch::Receiver<Option<T>>,
}

// read-only access, available to both readers and writers
impl<T: Value, const IS_WRITER: bool> Cell<T, IS_WRITER> {
	/// Read the current value of the cell.
	///
	/// Returns `None` if no value has been written yet.
	///
	/// Time: O(1)
	pub fn read(&self) -> Option<T> {
		self.data.borrow().clone()
	}

	/// Read the current value of the cell.
	///
	/// Returns `None` if no value has been written yet.
	///
	/// Time: O(1)
	pub fn get(&self) -> Option<T> {
		self.read()
	}

	/// Test whether the cell contains a value.
	///
	/// Time: O(1)
	pub fn is_empty(&self) -> bool {
		self.data.borrow().is_none()
	}

	/// Test whether the cell contains a value.
	///
	/// Time: O(1)
	pub fn is_none(&self) -> bool {
		self.is_empty()
	}

	/// Test whether the cell contains a value.
	///
	/// Time: O(1)
	pub fn is_some(&self) -> bool {
		!self.is_empty()
	}

	/// Returns an observer of the cell's state, which can be used to wait
	/// for the cell to reach a certain state version before performing an
	/// action or knowing when it is online or offline.
	pub const fn when(&self) -> &When {
		&self.when
	}

	/// The current version of the cell's state, which is the version of the
	/// latest committed state.
	pub fn version(&self) -> Version {
		Version(self.group.committed())
	}
}

// Mutable operations, only available to writers
impl<T: Value> CellWriter<T> {
	/// Create a new cell in writer mode.
	///
	/// The returned writer can be used to modify the cell, and it also
	/// provides read access to the cell's contents. Writers can be used by
	/// multiple nodes concurrently, and all changes made by any writer will be
	/// replicated to all other writers and readers.
	///
	/// This creates a new cell with default synchronization configuration.
	/// If you want to customize the synchronization behavior, use
	/// `writer_with_config` instead.
	///
	/// Note that different sync configurations will create different group ids
	/// and the resulting cells will not be able to see each other.
	pub fn writer(network: &Network, store_id: impl Into<StoreId>) -> Self {
		Self::writer_with_config(network, store_id, CollectionConfig::default())
	}

	/// Create a new cell in writer mode with the specified configuration.
	pub fn writer_with_config(
		network: &Network,
		store_id: impl Into<StoreId>,
		config: impl Into<CollectionConfig>,
	) -> Self {
		Self::create::<WRITER>(network, store_id, config.into())
	}

	/// Create a new cell in writer mode.
	///
	/// This is an alias for the `writer` method.
	pub fn new(network: &Network, store_id: impl Into<StoreId>) -> Self {
		Self::writer(network, store_id)
	}

	/// Create a new cell in writer mode with the specified configuration.
	///
	/// This is an alias for the `writer_with_config` method.
	pub fn new_with_config(
		network: &Network,
		store_id: impl Into<StoreId>,
		config: impl Into<CollectionConfig>,
	) -> Self {
		Self::writer_with_config(network, store_id, config)
	}

	/// Write a new value to the cell, replacing the previous one.
	///
	/// Time: O(1)
	pub fn write(
		&self,
		value: T,
	) -> impl Future<Output = Result<Version, Error<T>>> + Send + Sync + 'static
	{
		let value = Encoded(value);
		self.execute(
			CellCommand::Write { value },
			|cmd| match cmd {
				CellCommand::Write { value } => Error::Offline(value.0),
				_ => unreachable!(),
			},
			|cmd, e| match cmd {
				CellCommand::Write { value } => Error::Encoding(value.0, e),
				_ => unreachable!(),
			},
		)
	}

	/// Write a new value to the cell, replacing the previous one.
	///
	/// Time: O(1)
	pub fn set(
		&self,
		value: T,
	) -> impl Future<Output = Result<Version, Error<T>>> + Send + Sync + 'static
	{
		self.write(value)
	}

	/// Compare the current value of the cell with an expected value, and if
	/// they match, replace it with a new value. If the current value does not
	/// match the expected value, no write occurs.
	///
	/// Time: O(1)
	#[allow(clippy::type_complexity)]
	pub fn compare_exchange(
		&self,
		current: Option<T>,
		new: Option<T>,
	) -> impl Future<Output = Result<Version, Error<(Option<T>, Option<T>)>>>
	+ Send
	+ Sync
	+ 'static {
		let current = current.map(Encoded);
		let new = new.map(Encoded);

		self.execute(
			CellCommand::CompareExchange { current, new },
			|cmd| match cmd {
				CellCommand::CompareExchange { current, new } => {
					Error::Offline((current.map(|v| v.0), new.map(|v| v.0)))
				}
				_ => unreachable!(),
			},
			|cmd, e| match cmd {
				CellCommand::CompareExchange { current, new } => {
					Error::Encoding((current.map(|v| v.0), new.map(|v| v.0)), e)
				}
				_ => unreachable!(),
			},
		)
	}

	/// Clear the cell, removing the stored value.
	///
	/// After this operation, `read()` will return `None`.
	///
	/// Time: O(1)
	pub fn clear(
		&self,
	) -> impl Future<Output = Result<Version, Error<()>>> + Send + Sync + 'static
	{
		self.execute(
			CellCommand::Clear,
			|_| Error::Offline(()),
			|_, _| unreachable!(),
		)
	}
}

// construction
impl<T: Value, const IS_WRITER: bool> Cell<T, IS_WRITER> {
	/// Create a new cell in reader mode.
	///
	/// The returned reader provides read-only access to the cell's contents.
	/// Readers can be used by multiple nodes concurrently, and they will see all
	/// changes made by any writer. However, readers cannot modify the cell,
	/// and they will not be able to make any changes themselves. Readers have
	/// longer election timeouts to reduce the likelihood of them being elected
	/// as group leaders, which reduces latency for read operations.
	///
	/// This creates a new cell with the default sync configuration. If you
	/// want to specify a custom sync configuration, use the
	/// `reader_with_config` method instead.
	pub fn reader(
		network: &Network,
		store_id: impl Into<StoreId>,
	) -> CellReader<T> {
		Self::reader_with_config(network, store_id, CollectionConfig::default())
	}

	/// Create a new cell in reader mode with the specified configuration.
	pub fn reader_with_config(
		network: &Network,
		store_id: impl Into<StoreId>,
		config: impl Into<CollectionConfig>,
	) -> CellReader<T> {
		Self::create::<READER>(network, store_id, config.into())
	}

	fn create<const W: bool>(
		network: &Network,
		store_id: impl Into<StoreId>,
		config: CollectionConfig,
	) -> Cell<T, W> {
		let store_id = store_id.into();
		let machine = CellStateMachine::new(
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
		let when = When::new(group.when().clone());

		Cell::<T, W> { when, group, data }
	}
}

impl<T: Value, const WRITER: bool> CollectionFromDef for Cell<T, WRITER> {
	type Reader = CellReader<T>;
	type Writer = CellWriter<T>;

	fn reader_with_config(
		network: &Network,
		store_id: StoreId,
		config: CollectionConfig,
	) -> Self::Reader {
		Self::Reader::reader_with_config(network, store_id, config)
	}

	fn writer_with_config(
		network: &Network,
		store_id: StoreId,
		config: CollectionConfig,
	) -> Self::Writer {
		Self::Writer::writer_with_config(network, store_id, config)
	}
}

// internal
impl<T: Value> CellWriter<T> {
	fn execute<TErr>(
		&self,
		command: CellCommand<T>,
		offline_err: impl FnOnce(CellCommand<T>) -> Error<TErr> + Send + Sync + 'static,
		encoding_err: impl FnOnce(CellCommand<T>, EncodeError) -> Error<TErr>
		+ Send
		+ Sync
		+ 'static,
	) -> impl Future<Output = Result<Version, Error<TErr>>> + Send + Sync + 'static
	{
		self
			.group
			.execute(command)
			.map_err(|err| match err {
				CommandError::Offline(mut items) => offline_err(items.remove(0)),
				CommandError::Encoding(mut items, err) => {
					encoding_err(items.remove(0), err)
				}
				CommandError::GroupTerminated => Error::NetworkDown,
				CommandError::NoCommands => unreachable!(),
			})
			.map(|pos| pos.map(Version))
	}
}

struct CellStateMachine<T: Value> {
	data: Option<T>,
	latest: watch::Sender<Option<T>>,
	store_id: StoreId,
	local_id: PeerId,
	state_sync: SnapshotSync<Self>,
	is_writer: bool,
}

impl<T: Value> CellStateMachine<T> {
	pub fn new(
		store_id: StoreId,
		is_writer: bool,
		sync_config: SyncConfig,
		local_id: PeerId,
	) -> Self {
		let data = None;
		let state_sync = SnapshotSync::new(sync_config, |request| {
			CellCommand::TakeSnapshot(request)
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

impl<T: Value> StateMachine for CellStateMachine<T> {
	type Command = CellCommand<T>;
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
				CellCommand::Write { value } => {
					self.data = Some(value.0);
				}
				CellCommand::CompareExchange { current, new } => {
					if self.data.as_ref().map(|v| v.encode().ok())
						== current.map(|v| v.encode().ok())
					{
						self.data = new.map(|v| v.0);
					}
				}
				CellCommand::Clear => {
					self.data = None;
				}
				CellCommand::TakeSnapshot(request) => {
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

	/// The group-key for a cell is derived from the store ID and the type
	/// of the cell's value. This ensures that different cells (with
	/// different store IDs or value types) will be in different groups.
	fn signature(&self) -> crate::UniqueId {
		UniqueId::from("mosaik_collections_cell")
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

	/// Readers are observers and never assume group leadership.
	fn leadership_preference(&self) -> LeadershipPreference {
		if self.is_writer {
			LeadershipPreference::Normal
		} else {
			LeadershipPreference::Observer
		}
	}
}

impl<T: Value> SnapshotStateMachine for CellStateMachine<T> {
	type Snapshot = CellSnapshot<T>;

	fn create_snapshot(&self) -> Self::Snapshot {
		CellSnapshot {
			data: self.data.clone().map(Encoded),
		}
	}

	fn install_snapshot(&mut self, snapshot: Self::Snapshot) {
		self.data = snapshot.data.map(|d| d.0);
		self.latest.send_replace(self.data.clone());
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "T: Value")]
enum CellCommand<T> {
	Write {
		value: Encoded<T>,
	},
	CompareExchange {
		current: Option<Encoded<T>>,
		new: Option<Encoded<T>>,
	},
	Clear,
	TakeSnapshot(SnapshotRequest),
}

#[derive(Debug, Clone)]
pub struct CellSnapshot<T: Value> {
	data: Option<Encoded<T>>,
}

impl<T: Value> Default for CellSnapshot<T> {
	fn default() -> Self {
		Self { data: None }
	}
}

impl<T: Value> Snapshot for CellSnapshot<T> {
	type Item = Encoded<T>;

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
		// For a cell, the last item wins
		for item in items {
			self.data = Some(item);
		}
	}
}
