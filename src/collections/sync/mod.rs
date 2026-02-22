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
	inner::SnapshotSyncInner,
	provider::SnapshotSyncProvider,
	serde::{Deserialize, Serialize, de::DeserializeOwned},
	session::SnapshotSyncSession,
	std::sync::Arc,
	tokio::sync::{broadcast, mpsc::UnboundedSender},
};

mod inner;
mod provider;
mod session;
mod snapshot;

use parking_lot::RwLock;
pub use snapshot::{Snapshot, SnapshotItem};

/// State sync strategy for mosaik collections that restores the state of a
/// lagging follower by syncing data from a snapshot.
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
pub struct SnapshotSync<M: SnapshotStateMachine>(
	Arc<RwLock<SnapshotSyncInner<M>>>,
);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotRequest {
	pub requested_by: PeerId,
	pub requested_at: DateTime<Utc>,
}

/// Extension of the `StateMachine` trait that requires the state machine to be
/// able to create snapshots.
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

#[derive(Debug, Clone)]
pub struct Config {
	/// The batch size for snapshot data transfer during the sync process.
	fetch_batch_size: u64,

	/// The time-to-live for snapshot requests.
	snapshot_ttl: Duration,

	/// How long to wait for a `SnapshotReady` response before retrying the
	/// `RequestSnapshot` message to the (possibly new) leader.
	snapshot_request_timeout: Duration,

	/// How long to wait for a `FetchDataResponse` before considering the
	/// request timed out and removing the peer from availability.
	fetch_timeout: Duration,
}

impl Config {
	/// The batch size for snapshot data transfer during the sync process. This
	/// is the number of individual data items contained in a snapshot that will
	/// be transferred in a single message during the sync process.
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

	/// The time-to-live for snapshot requests. If a snapshot command is applied
	/// to the log at a timestamp that is this later than the timestamp of
	/// the snapshot request by this duration, the provider should ignore the
	/// request and not create a snapshot for it.
	///
	/// Snapshots on providers are discarded after they have been inactive for
	/// this duration as well. Each fetch request for a snapshot will reset the
	/// timer for that snapshot.
	#[must_use]
	pub const fn with_snapshot_request_ttl(
		mut self,
		snapshot_request_ttl: Duration,
	) -> Self {
		self.snapshot_ttl = snapshot_request_ttl;
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

#[derive(Clone, Serialize, Deserialize, From)]
#[serde(bound = "T: SnapshotItem")]
pub enum SnapshotSyncMessage<T> {
	RequestSnapshot,
	SnapshotReady(SnapshotInfo),
	FetchDataRequest(FetchDataRequest),
	FetchDataResponse(FetchDataResponse<T>),
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
pub struct SnapshotInfo {
	pub anchor: Cursor,
	pub len: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchDataRequest {
	pub anchor: Cursor,
	pub range: Range<u64>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(bound = "T: SnapshotItem")]
pub struct FetchDataResponse<T> {
	pub anchor: Cursor,
	pub offset: u64,
	pub items: Vec<T>,
}
