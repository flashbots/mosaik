use {
	super::{
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

/// Mutable access to a replicated write-once register.
///
/// Has higher priority for assuming group leadership.
pub type OnceWriter<T> = Once<T, WRITER>;

/// Read-only access to a write-once register.
///
/// Has lower priority for assuming group leadership.
pub type OnceReader<T> = Once<T, READER>;

/// Replicated write-once register.
///
/// A `Once` holds at most one value. Unlike [`Register`], the value can only
/// be set once — subsequent writes are silently ignored by the state machine.
/// This is the distributed equivalent of a `tokio::sync::OnceCell`.
///
/// [`Register`]: super::Register
pub struct Once<T: Value, const IS_WRITER: bool = WRITER> {
	when: When,
	group: Group<OnceStateMachine<T>>,
	data: watch::Receiver<Option<T>>,
}

// read-only access, available to both readers and writers
impl<T: Value, const IS_WRITER: bool> Once<T, IS_WRITER> {
	/// Read the current value.
	///
	/// Returns `None` if no value has been written yet.
	///
	/// Time: O(1)
	pub fn read(&self) -> Option<T> {
		self.data.borrow().clone()
	}

	/// Read the current value.
	///
	/// Returns `None` if no value has been written yet.
	///
	/// Time: O(1)
	pub fn get(&self) -> Option<T> {
		self.read()
	}

	/// Test whether the register has been set.
	///
	/// Time: O(1)
	pub fn is_empty(&self) -> bool {
		self.data.borrow().is_none()
	}

	/// Test whether the register has been set.
	///
	/// Time: O(1)
	pub fn is_none(&self) -> bool {
		self.is_empty()
	}

	/// Test whether the register has been set.
	///
	/// Time: O(1)
	pub fn is_some(&self) -> bool {
		!self.is_empty()
	}

	/// Returns an observer of the register's state, which can be used to wait
	/// for it to reach a certain state version before performing an action or
	/// knowing when it is online or offline.
	pub const fn when(&self) -> &When {
		&self.when
	}

	/// The current version of the register's state, which is the version of
	/// the latest committed state.
	pub fn version(&self) -> Version {
		Version(self.group.committed())
	}

	/// Waits for the register to be set and returns its value.
	pub async fn await_value(&self) -> T {
		self.when().online().await;

		loop {
			let updated_fut = self.when().updated();
			if let Some(value) = self.read() {
				return value;
			}
			updated_fut.await;
		}
	}
}

// Mutable operations, only available to writers
impl<T: Value> OnceWriter<T> {
	/// Create a new write-once register in writer mode.
	///
	/// This creates a new register with default synchronization configuration.
	/// If you want to customize the synchronization behavior, use
	/// `writer_with_config` instead.
	pub fn writer(network: &Network, store_id: impl Into<StoreId>) -> Self {
		Self::writer_with_config(network, store_id, SyncConfig::default())
	}

	/// Create a new write-once register in writer mode with the specified sync
	/// configuration.
	pub fn writer_with_config(
		network: &Network,
		store_id: impl Into<StoreId>,
		config: SyncConfig,
	) -> Self {
		Self::create::<WRITER>(network, store_id, config)
	}

	/// Create a new write-once register in writer mode.
	///
	/// This is an alias for the `writer` method.
	pub fn new(network: &Network, store_id: impl Into<StoreId>) -> Self {
		Self::writer(network, store_id)
	}

	/// Create a new write-once register in writer mode with the specified sync
	/// configuration.
	///
	/// This is an alias for the `writer_with_config` method.
	pub fn new_with_config(
		network: &Network,
		store_id: impl Into<StoreId>,
		config: SyncConfig,
	) -> Self {
		Self::writer_with_config(network, store_id, config)
	}

	/// Set the value of the register.
	///
	/// The value is only written if the register is currently empty. If a
	/// value has already been set, this operation is silently ignored by the
	/// state machine — the returned `Version` still advances, but the stored
	/// value does not change.
	///
	/// Time: O(1)
	pub fn write(
		&self,
		value: T,
	) -> impl Future<Output = Result<Version, Error<T>>> + Send + Sync + 'static
	{
		let value = Encoded(value);
		self.execute(
			OnceCommand::Write { value },
			|cmd| match cmd {
				OnceCommand::Write { value } => Error::Offline(value.0),
				OnceCommand::TakeSnapshot(_) => unreachable!(),
			},
			|cmd, e| match cmd {
				OnceCommand::Write { value } => Error::Encoding(value.0, e),
				OnceCommand::TakeSnapshot(_) => unreachable!(),
			},
		)
	}

	/// Set the value of the register.
	///
	/// This is an alias for the `write` method.
	///
	/// Time: O(1)
	pub fn set(
		&self,
		value: T,
	) -> impl Future<Output = Result<Version, Error<T>>> + Send + Sync + 'static
	{
		self.write(value)
	}
}

// construction
impl<T: Value, const IS_WRITER: bool> Once<T, IS_WRITER> {
	/// Create a new write-once register in reader mode.
	///
	/// The returned reader provides read-only access to the register's
	/// contents. Readers have longer election timeouts to reduce the
	/// likelihood of them being elected as group leaders.
	pub fn reader(
		network: &Network,
		store_id: impl Into<StoreId>,
	) -> OnceReader<T> {
		Self::reader_with_config(network, store_id, SyncConfig::default())
	}

	/// Create a new write-once register in reader mode with the specified sync
	/// configuration.
	pub fn reader_with_config(
		network: &Network,
		store_id: impl Into<StoreId>,
		config: SyncConfig,
	) -> OnceReader<T> {
		Self::create::<READER>(network, store_id, config)
	}

	fn create<const W: bool>(
		network: &Network,
		store_id: impl Into<StoreId>,
		config: SyncConfig,
	) -> Once<T, W> {
		let store_id = store_id.into();
		let machine = OnceStateMachine::new(
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

		Once::<T, W> { when, group, data }
	}
}

impl<T: Value, const WRITER: bool> CollectionFromDef for Once<T, WRITER> {
	type Reader = OnceReader<T>;
	type Writer = OnceWriter<T>;

	fn reader(network: &Network, store_id: StoreId) -> Self::Reader {
		Self::Reader::reader(network, store_id)
	}

	fn writer(network: &Network, store_id: StoreId) -> Self::Writer {
		Self::Writer::writer(network, store_id)
	}
}

// internal
impl<T: Value> OnceWriter<T> {
	fn execute<TErr>(
		&self,
		command: OnceCommand<T>,
		offline_err: impl FnOnce(OnceCommand<T>) -> Error<TErr> + Send + Sync + 'static,
		encoding_err: impl FnOnce(OnceCommand<T>, EncodeError) -> Error<TErr>
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

struct OnceStateMachine<T: Value> {
	data: Option<T>,
	latest: watch::Sender<Option<T>>,
	store_id: StoreId,
	local_id: PeerId,
	state_sync: SnapshotSync<Self>,
	is_writer: bool,
}

impl<T: Value> OnceStateMachine<T> {
	pub fn new(
		store_id: StoreId,
		is_writer: bool,
		sync_config: SyncConfig,
		local_id: PeerId,
	) -> Self {
		let data = None;
		let state_sync = SnapshotSync::new(sync_config, |request| {
			OnceCommand::TakeSnapshot(request)
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

impl<T: Value> StateMachine for OnceStateMachine<T> {
	type Command = OnceCommand<T>;
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
				OnceCommand::Write { value } => {
					// Only accept the first write
					if self.data.is_none() {
						self.data = Some(value.0);
					}
				}
				OnceCommand::TakeSnapshot(request) => {
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

	fn signature(&self) -> crate::UniqueId {
		UniqueId::from("mosaik_collections_once")
			.derive(self.store_id)
			.derive(type_name::<T>())
	}

	fn query(&self, (): Self::Query) {}

	fn state_sync(&self) -> Self::StateSync {
		self.state_sync.clone()
	}

	fn leadership_preference(&self) -> LeadershipPreference {
		if self.is_writer {
			LeadershipPreference::Normal
		} else {
			LeadershipPreference::Observer
		}
	}
}

impl<T: Value> SnapshotStateMachine for OnceStateMachine<T> {
	type Snapshot = OnceSnapshot<T>;

	fn create_snapshot(&self) -> Self::Snapshot {
		OnceSnapshot {
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
enum OnceCommand<T> {
	Write { value: Encoded<T> },
	TakeSnapshot(SnapshotRequest),
}

#[derive(Debug, Clone)]
pub struct OnceSnapshot<T: Value> {
	data: Option<Encoded<T>>,
}

impl<T: Value> Default for OnceSnapshot<T> {
	fn default() -> Self {
		Self { data: None }
	}
}

impl<T: Value> Snapshot for OnceSnapshot<T> {
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
		for item in items {
			self.data = Some(item);
		}
	}
}
