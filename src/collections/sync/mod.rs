use {
	crate::{PeerId, UniqueId, groups::*},
	core::{
		any::type_name,
		marker::PhantomData,
		task::{Context, Poll},
	},
	provider::SnapshotSyncProvider,
	serde::{Deserialize, Serialize, de::DeserializeOwned},
	session::SnapshotSyncSession,
};

mod provider;
mod session;
mod snapshot;

pub(in crate::collections) use snapshot::{Snapshot, SnapshotItem};

/// State sync strategy for mosaik collections that restores the state of a
/// lagging follower by syncing data from a known snapshot then replaying
/// tailing log entries.
///
/// Notes:
///
/// - This strategy does not replay the entire log from the beginning. Instead,
///   it relies on the state machine to provide snapshots of its state at
///   regular intervals, which can be used to quickly restore the state of a
///   lagging follower without having to replay the entire commands log.
///
/// - State sync falls back to log-replay if the distance between the lagging
///   follower is not too large or if there are no snapshots available for
///   syncing. This ensures that new nodes can still
pub struct SnapshotSync<M: SnapshotStateMachine> {
	config: Config,
	_p: PhantomData<M>,
}

impl<M: SnapshotStateMachine> Default for SnapshotSync<M> {
	fn default() -> Self {
		Self::new(Config::default())
	}
}

impl<M: SnapshotStateMachine> SnapshotSync<M> {
	pub(super) const fn new(config: Config) -> Self {
		Self {
			config,
			_p: PhantomData,
		}
	}

	#[must_use]
	pub(super) const fn with_interval(mut self, interval: u64) -> Self {
		self.config.interval = interval;
		self
	}

	#[must_use]
	pub(super) const fn with_window_size(mut self, window_size: u32) -> Self {
		self.config.retention_window = window_size;
		self
	}
}

impl<M: SnapshotStateMachine> StateSync for SnapshotSync<M> {
	type Machine = M;
	type Message = SnapshotSyncMessage;
	type Provider = provider::SnapshotSyncProvider<M>;
	type Session = session::SnapshotSyncSession<M>;

	fn signature(&self) -> crate::UniqueId {
		UniqueId::from("mosaik_collections_snapshot_sync")
			.derive(type_name::<M>())
			.derive(self.config.interval.to_le_bytes())
			.derive(self.config.retention_window.to_le_bytes())
	}

	fn create_provider(&self, cx: &dyn StateSyncContext<Self>) -> Self::Provider {
		SnapshotSyncProvider::new(self.config.clone(), cx)
	}

	fn create_session(
		&self,
		cx: &mut dyn StateSyncContext<Self>,
		position: Cursor,
		leader_commit: Index,
		entries: Vec<(M::Command, Term)>,
	) -> Self::Session {
		SnapshotSyncSession::new(&self.config, cx, position, leader_commit, entries)
	}
}

/// Extension of the `StateMachine` trait that requires the state machine to be
/// able to create snapshots.
pub(super) trait SnapshotStateMachine: StateMachine {
	/// The type that represents the state of the state machine at a specific
	/// point in time.
	type Snapshot: Snapshot;

	/// This should be a cheap operation and in the case of mosaik collections, it
	/// is O(1) as the underlying data structures used by the state machine are
	/// immutable (`im` crate) and support cheap cloning at specific points in
	/// time.
	fn snapshot(
		&self,
		cx: &dyn StateSyncContext<SnapshotSync<Self>>,
	) -> Self::Snapshot;
}

#[derive(Debug, Clone)]
pub struct Config {
	/// The index interval at which the state machine should create snapshots of
	/// its state.
	///
	/// That is every `interval` committed log entries, the state
	/// machine should create a new snapshot of its state and make it available
	/// to sync sessions.
	interval: u64,

	/// A rolling window size that determines how many of the most recent
	/// snapshots the provider should always have available for new sync
	/// sessions.
	retention_window: u32,

	/// The batch size for snapshot data transfer during the sync process. This
	/// is the number of individual data items contained in a snapshot that will
	/// be transferred in a single message during the sync process.
	///
	/// Try to keep this value in the range that would give us individual batches
	/// in the range of 2MB-5MB.
	batch_size: u64,
}

impl Config {
	/// Set the snapshot creation interval.
	#[must_use]
	pub const fn with_interval(mut self, interval: u64) -> Self {
		self.interval = interval;
		self
	}

	/// Set the rolling window size for available snapshots.
	#[must_use]
	pub const fn with_retention_window(mut self, retention_window: u32) -> Self {
		self.retention_window = retention_window;
		self
	}

	/// Set the batch size for snapshot data transfer during the sync process.
	#[must_use]
	pub const fn with_batch_size(mut self, batch_size: u64) -> Self {
		self.batch_size = batch_size;
		self
	}
}

impl Default for Config {
	fn default() -> Self {
		Self {
			interval: 5000,
			retention_window: 5,
			batch_size: 200,
		}
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SnapshotSyncMessage {
	PrepareRequest {
		position: Index,
		request_id: u64,
	},
	PrepareResponse {
		available: Vec<Index>,
		request_id: u64,
	},
}
