use {
	crate::{PeerId, UniqueId, groups::*, primitives::UnboundedChannel},
	chrono::{DateTime, Utc},
	core::{
		any::type_name,
		marker::PhantomData,
		ops::Range,
		task::{Context, Poll},
		time::Duration,
	},
	derive_more::From,
	parking_lot::RwLock,
	protocol::{SnapshotRequest, SnapshotSyncMessage},
	provider::SnapshotSyncProvider,
	serde::{Deserialize, Serialize, de::DeserializeOwned},
	session::SnapshotSyncSession,
	std::sync::Arc,
	tokio::sync::{
		broadcast,
		mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
	},
};

pub(super) mod protocol;
mod provider;
mod session;

/// State sync strategy for mosaik collections that restores the state of a
/// lagging follower by syncing data from a snapshot at a specific anchor state
/// log position.
///
/// The mechanism of this state sync strategy is as follows:
///
/// 1. When a follower detects that it is lagging behind the leader it will send
///    a `RequestSnapshot` message to the leader.
///
/// 2. Upon receiving a `RequestSnapshot` message, the leader will create a
///    `SnapshotRequest` instance containing the ID of the requesting peer and
///    the timestamp of the request, then translate it to a
///    state-machine-specific command using the provided `SyncInitCommand` and
///    feed that command to the group for replication at a later position in the
///    log that will be seen by all peers in the group.
///
/// 3. When the snapshot command is applied and committed to the log, all peers
///    will see the command at the same position in the log and will create a
///    snapshot of their state at the same position then send a `SnapshotReady`
///    message to the requesting peer containing the position of the snapshot.
///
///    3.a. If the timestamp of the `SnapshotRequest` is older than the
///    configured `snapshot_request_ttl`, it will be ignored and no snapshot
///    will be created for it. This prevents peers from creating snapshots for
///    stale requests for example when they are syncing historical state.
///
///    3.b. In state machine implementations that implement their own
///    `apply_batch` method, they should take the snapshot at the position of
///    the end of the batch of commands that contains the snapshot command, so
///    that the snapshot reflects the state after applying that command and all
///    previous commands in the log.
///
/// 4. Upon receiving `SnapshotReady` messages from peers, the requesting
///    lagging follower will start fetching the snapshot data in batches by
///    sending `FetchDataRequest` messages to the peers that have the snapshot
///    ready.
///
/// 5. The peers will respond to `FetchDataRequest` messages with
///    `FetchDataResponse` messages containing the requested batch of snapshot
///    data items.
///
/// 6. Once the lagging follower has fetched all batches of snapshot data, it
///    will apply the snapshot to its state and apply all buffered commands that
///    it received during the sync process but has not applied yet. At this
///    point the follower is fully synced and can start applying new commands
///    from the log as they come in.
///
/// Notes:
///
///  - This state sync strategy always sync the full state of the group until a
///    specific log position, it does not support syncing partial state or
///    between two log positions.
pub struct SnapshotSync<M: SnapshotStateMachine>(
	Arc<RwLock<SnapshotSyncInner<M>>>,
);

/// Extension of the `StateMachine` trait that requires the state machine to be
/// able to create and install snapshots.
pub trait SnapshotStateMachine: StateMachine {
	/// The type that represents the state of the state machine at a specific
	/// point in time.
	type Snapshot: Snapshot;

	/// This should be a cheap operation and in the case of mosaik collections, it
	/// is O(1) as the underlying data structures used by the state machine are
	/// immutable (`im` crate) and support cheap cloning at specific points in
	/// time.
	fn create_snapshot(&self) -> Self::Snapshot;

	/// Applies the given snapshot to the state machine, replacing its current
	/// state with the state represented by the snapshot. This is called by the
	/// [`SnapshotSyncSession`] once it has fetched all snapshot data.
	fn install_snapshot(&mut self, snapshot: Self::Snapshot);
}

/// Represents a snapshot of the state of the state machine at a specific point
/// in time. This is used to quickly restore the state of a lagging follower
/// without having to replay the entire log.
///
/// When instantiating a default snapshot, it should return an empty snapshot
/// with no items.
pub trait Snapshot: Default + Clone + Send + 'static {
	/// Type of the individual data items contained in this snapshot.
	type Item: SnapshotItem;

	/// Returns `true` if this snapshot contains no data items.
	fn is_empty(&self) -> bool {
		self.len() == 0
	}

	/// The number of individual data items contained in this snapshot.
	///
	/// E.g. in `Vec` it will be the number of elements, in `Map` it will be the
	/// number of key-value pairs, etc. This is used to chunk the snapshot into
	/// smaller pieces and distributing requests for those pieces across multiple
	/// peers during the sync process.
	fn len(&self) -> u64;

	/// Returns an iterator over the individual data items contained in this
	/// snapshot.
	///
	/// This iterator must always return the same items in the same order for the
	/// same snapshot on all peers for a given range.
	///
	/// Implementation should either return the full range of items or `None`.
	fn iter_range(
		&self,
		range: Range<u64>,
	) -> Option<impl Iterator<Item = Self::Item>>;

	/// Appends the given items to the snapshot.
	///
	/// It is guaranteed that the given items are the next items in the snapshot
	/// after the current last item, so the implementation can simply append them
	/// to the end of the snapshot.
	fn append(&mut self, items: impl IntoIterator<Item = Self::Item>);
}

/// Represents an individual data item contained in a snapshot. This is used to
/// define the type of items that are being transferred during the snapshot sync
/// process.
///
/// This does not have to be the same type as the items contained in the state
/// machine's state.
pub trait SnapshotItem:
	Clone + Send + Serialize + DeserializeOwned + 'static
{
}

impl<T> SnapshotItem for T where
	T: Clone + Send + Serialize + DeserializeOwned + 'static
{
}

#[derive(Debug, Clone)]
pub struct Config {
	/// The batch size for individual snapshot items transfer during the sync
	/// process using the `FetchDataRequest`/`FetchDataResponse` messages.
	fetch_batch_size: u64,

	/// The time-to-live for active snapshots.
	///
	/// Snapshots are considered active from the moment they were last interacted
	/// with (either created or fetched from) and until they have been inactive
	/// for this duration. Providers will ignore sync requests that are older
	/// than this duration and will discard snapshots that have been inactive for
	/// this duration as well.
	snapshot_ttl: Duration,

	/// How long to wait for a `SnapshotReady` response before retrying the
	/// `RequestSnapshot` message to the (possibly new) leader.
	snapshot_request_timeout: Duration,

	/// How long to wait for a `FetchDataResponse` before considering the
	/// request timed out and removing the peer from availability.
	fetch_timeout: Duration,
}

impl Config {
	/// The batch size for individual snapshot items transfer during the sync
	/// process using the `FetchDataRequest`/`FetchDataResponse` messages.
	///
	/// Try to keep this value in the range that would give us individual batches
	/// in the range of 2MB-5MB or for the fetch transfer to not exceed the
	/// snapshot TTL, otherwise the provider might discard the snapshot before
	/// the follower can finish fetching it and it stops being available.
	#[must_use]
	pub const fn with_fetch_batch_size(mut self, fetch_batch_size: u64) -> Self {
		self.fetch_batch_size = fetch_batch_size;
		self
	}

	/// The time-to-live for active snapshot.
	///
	/// Snapshots are considered active from the moment they were last interacted
	/// with (either created or fetched from) and until they have been inactive
	/// for this duration. Providers will ignore sync requests that are older
	/// than this duration and will discard snapshots that have been inactive for
	/// this duration as well.
	#[must_use]
	pub const fn with_snapshot_ttl(mut self, snapshot_ttl: Duration) -> Self {
		self.snapshot_ttl = snapshot_ttl;
		self
	}

	/// How long to wait for a `SnapshotReady` response from the leader before
	/// retrying the `RequestSnapshot` message. This timeout resets each time a
	/// retry is sent and is disabled once the first `SnapshotReady` arrives.
	#[must_use]
	pub const fn with_snapshot_request_timeout(
		mut self,
		snapshot_request_timeout: Duration,
	) -> Self {
		self.snapshot_request_timeout = snapshot_request_timeout;
		self
	}

	/// How long to wait for a `FetchDataResponse` from a peer before
	/// considering the in-flight request timed out and removing the peer
	/// from availability tracking.
	#[must_use]
	pub const fn with_fetch_timeout(mut self, fetch_timeout: Duration) -> Self {
		self.fetch_timeout = fetch_timeout;
		self
	}
}

impl Config {
	/// Returns true if the given snapshot request is considered stale and should
	/// be ignored by the provider when it is replicated in the log, false
	/// otherwise.
	pub(super) fn is_expired(&self, request: &SnapshotRequest) -> bool {
		let Ok(elapsed) = Utc::now()
			.signed_duration_since(request.requested_at)
			.abs()
			.to_std()
		else {
			// if the request timestamp is in the future for some reason, consider it
			// expired to be safe.
			return true;
		};

		elapsed > self.snapshot_ttl
	}
}

impl Default for Config {
	fn default() -> Self {
		Self {
			fetch_batch_size: 200,
			snapshot_ttl: Duration::from_secs(10),
			snapshot_request_timeout: Duration::from_secs(15),
			fetch_timeout: Duration::from_secs(5),
		}
	}
}

/// Translates a snapshot request into a state-machine-specific command that can
/// trigger the creation of a snapshot when applied to the log.
pub type SyncInitCommand<M: SnapshotStateMachine> =
	Arc<dyn Fn(SnapshotRequest) -> M::Command + Send + Sync>;

// construction
impl<M: SnapshotStateMachine> SnapshotSync<M> {
	/// Creates a new instance of the snapshot sync strategy with the given config
	/// and a function that translates snapshot requests to state-machine-specific
	/// marker commands that will be replicated in the log to trigger snapshot
	/// creation.
	pub(super) fn new(
		config: Config,
		to_command: impl Fn(SnapshotRequest) -> M::Command + Send + Sync + 'static,
	) -> Self {
		let inner = SnapshotSyncInner::new(config, to_command);
		Self(Arc::new(RwLock::new(inner)))
	}
}

impl<M: SnapshotStateMachine> Clone for SnapshotSync<M> {
	fn clone(&self) -> Self {
		Self(Arc::clone(&self.0))
	}
}

impl<M: SnapshotStateMachine> StateSync for SnapshotSync<M> {
	type Machine = M;
	type Message = SnapshotSyncMessage<<M::Snapshot as Snapshot>::Item>;
	type Provider = provider::SnapshotSyncProvider<M>;
	type Session = session::SnapshotSyncSession<M>;

	fn signature(&self) -> crate::UniqueId {
		self.0.read().signature()
	}

	fn create_provider(&self, cx: &dyn SyncContext<Self>) -> Self::Provider {
		self.0.write().create_provider(cx)
	}

	fn create_session(
		&self,
		cx: &mut dyn SyncSessionContext<Self>,
		position: Cursor,
		leader_commit: Index,
		entries: Vec<(M::Command, Term)>,
	) -> Self::Session {
		self
			.0
			.read()
			.create_session(cx, position, leader_commit, entries)
	}
}

impl<M: SnapshotStateMachine> SnapshotSync<M> {
	/// Returns true if the given snapshot request is expired and should be
	/// ignored by the provider, false otherwise.
	pub fn is_expired(&self, request: &SnapshotRequest) -> bool {
		self.0.read().is_expired(request)
	}

	pub fn serve_snapshot(
		&self,
		request: SnapshotRequest,
		position: Cursor,
		snapshot: M::Snapshot,
	) {
		self.0.read().serve_snapshot(request, position, snapshot);
	}
}

struct PendingRequest<M: SnapshotStateMachine> {
	request: SnapshotRequest,
	position: Cursor,
	snapshot: M::Snapshot,
}

struct SnapshotSyncInner<M: SnapshotStateMachine> {
	config: Config,
	to_command: SyncInitCommand<M>,
	requests_tx: UnboundedSender<PendingRequest<M>>,
	requests_rx: Option<UnboundedReceiver<PendingRequest<M>>>,
}

impl<M: SnapshotStateMachine> SnapshotSyncInner<M> {
	pub fn new(
		config: Config,
		to_command: impl Fn(SnapshotRequest) -> M::Command + Send + Sync + 'static,
	) -> Self {
		let (requests_tx, requests_rx) = unbounded_channel();
		Self {
			config,
			requests_tx,
			requests_rx: Some(requests_rx),
			to_command: Arc::new(to_command),
		}
	}

	pub fn is_expired(&self, request: &SnapshotRequest) -> bool {
		self.config.is_expired(request)
	}

	pub fn serve_snapshot(
		&self,
		request: SnapshotRequest,
		position: Cursor,
		snapshot: M::Snapshot,
	) {
		let _ = self.requests_tx.send(PendingRequest {
			request,
			position,
			snapshot,
		});
	}
}

// StateSync implementation
impl<M: SnapshotStateMachine> SnapshotSyncInner<M> {
	pub fn signature(&self) -> crate::UniqueId {
		UniqueId::from("mosaik_collections_snapshot_sync")
			.derive(type_name::<M>())
			.derive(self.config.fetch_batch_size.to_le_bytes())
			.derive(self.config.snapshot_ttl.as_millis().to_le_bytes())
			.derive(
				self
					.config
					.snapshot_request_timeout
					.as_millis()
					.to_le_bytes(),
			)
			.derive(self.config.fetch_timeout.as_millis().to_le_bytes())
	}

	pub fn create_provider(
		&mut self,
		_cx: &dyn SyncContext<SnapshotSync<M>>,
	) -> SnapshotSyncProvider<M> {
		let Some(requests_rx) = self.requests_rx.take() else {
			unreachable!("create_provider called more than once. this is a bug.")
		};

		SnapshotSyncProvider::<M>::new(
			self.config.clone(),
			self.to_command.clone(),
			requests_rx,
		)
	}

	pub fn create_session(
		&self,
		cx: &mut dyn SyncSessionContext<SnapshotSync<M>>,
		position: Cursor,
		leader_commit: Index,
		entries: Vec<(M::Command, Term)>,
	) -> SnapshotSyncSession<M> {
		SnapshotSyncSession::new(&self.config, cx, position, leader_commit, entries)
	}
}
