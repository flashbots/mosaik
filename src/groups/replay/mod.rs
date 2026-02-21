use {
	crate::{
		groups::{
			Command,
			Cursor,
			Index,
			StateMachine,
			SyncContext,
			Term,
			machine::*,
		},
		primitives::UniqueId,
	},
	core::{
		any::type_name,
		marker::PhantomData,
		num::NonZero,
		ops::RangeInclusive,
		time::Duration,
	},
	provider::LogReplayProvider,
	serde::{Deserialize, Serialize},
	session::LogReplaySession,
};

mod provider;
mod session;

/// Generic log-replay state sync implementation
///
/// This implementation can be used with any state machine, it replays the
/// entire log from the beginning to synchronize a new node with the current
/// state of the group.
///
/// This is a simple and robust sync strategy that does not require any
/// additional state or snapshots, but it can be inefficient for large logs. It
/// is best suited for small groups with low command throughput, or for testing
/// and debugging purposes.
///
/// Notes:
/// - Logs will be replayed by all members of the group, including the leader.
///
/// - Syncing from the leader is deprioritized to avoid overloading the leader
///   with sync requests, but it is not strictly avoided. If no followers are
///   available for syncing, the new node may sync from the leader.
///
/// - The lagging follower will send `AvailabilityRequest` messages to all
///   bonded peers to discover which peers have the log entries it is missing
///   and to coordinate the catch-up process. Peers will respond with
///   `AvailabilityResponse` messages to inform the lagging follower of the
///   available log entries.
///
/// - The lagging follower will then send `FetchEntitiesRequest` messages to
///   individual peers to request log entries in the specified range during the
///   catch-up process. The size of these ranges is determined by the
///   `batch_size` parameter of this struct, which controls the maximum number
///   of log entries to replay in a single sync message. The follower will
///   continue to request batches of log entries until it has caught up with the
///   current state of the group.
///
/// - There will be at most one in-flight sync request from the lagging follower
///   to any given peer at a time, and the follower will wait for the response
///   before sending the next sync request to the same peer. This ensures that
///   the follower does not overwhelm any single peer with too many sync
///   requests at once.
///
/// - This state sync provider does not support pruning log entries, for
///   high-throughput groups using this state sync implementation consider using
///   on-disk logs storage.
pub struct LogReplaySync<M: StateMachine> {
	/// Configuration parameters for the log replay sync process.
	/// Those parameters contribute to the signature of this state sync
	/// implementation and must be identical across all members of the same
	/// group.
	config: Config,

	#[doc(hidden)]
	_marker: PhantomData<M>,
}

impl<M: StateMachine> Default for LogReplaySync<M> {
	fn default() -> Self {
		Self {
			config: Config::default(),
			_marker: PhantomData,
		}
	}
}

impl<M: StateMachine> core::fmt::Debug for LogReplaySync<M> {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		f.debug_struct("LogReplaySync")
			.field("config", &self.config)
			.finish()
	}
}

impl<M: StateMachine> StateSync for LogReplaySync<M> {
	type Machine = M;
	type Message = LogReplaySyncMessage<M::Command>;
	type Provider = LogReplayProvider<M>;
	type Session = LogReplaySession<M>;

	fn signature(&self) -> UniqueId {
		UniqueId::from("mosaik_log_replay_sync")
			.derive(type_name::<M>())
			.derive(self.config.batch_size.to_le_bytes())
			.derive(self.config.fetch_timeout.as_secs().to_le_bytes())
	}

	fn create_provider(&self, cx: &dyn SyncContext<Self>) -> Self::Provider {
		LogReplayProvider::new(&self.config, cx)
	}

	fn create_session(
		&self,
		cx: &mut dyn SyncSessionContext<Self>,
		position: Cursor,
		_leader_committed: Index,
		entries: Vec<(M::Command, Term)>,
	) -> Self::Session {
		LogReplaySession::new(&self.config, cx, position, entries)
	}
}

impl<M: StateMachine> LogReplaySync<M> {
	/// Sets the maximum number of log entries to replay in a single sync message.
	#[must_use]
	pub const fn with_batch_size(mut self, batch_size: NonZero<u64>) -> Self {
		self.config.batch_size = batch_size.get();
		self
	}

	/// Sets the timeout duration for fetching missing log entries during
	/// catch-up.
	#[must_use]
	pub const fn with_fetch_timeout(mut self, fetch_timeout: Duration) -> Self {
		self.config.fetch_timeout = fetch_timeout;
		self
	}
}

/// Messages exchanged on the wire between peers during the log-replay state
/// synchronization process.
#[derive(Clone, Serialize, Deserialize)]
#[serde(bound(deserialize = "C: Command"))]
pub enum LogReplaySyncMessage<C> {
	/// Message broadcasted by a lagging follower to all bonded peers to discover
	/// which peers have the log entries it is missing and to coordinate the
	/// catch-up process.
	AvailabilityRequest,

	/// Message sent by peers in response to a `AvailabilityRequest` message to
	/// inform the lagging follower of the range of log entries they have, which
	/// the follower can use to determine which peers to fetch which log entries
	/// from during the catch-up process.
	AvailabilityResponse(RangeInclusive<Index>),

	/// Message sent to individual peers to request log entries in the
	/// specified range during the catch-up process.
	FetchEntriesRequest(RangeInclusive<Index>),

	/// Message sent by peers in response to a `FetchEntriesRequest` message to
	/// provide the requested log entries during the catch-up process.
	FetchEntriesResponse {
		range: RangeInclusive<Index>,
		entries: Vec<(C, Term)>,
	},
}

#[derive(Debug, Clone)]
struct Config {
	/// The maximum number of log entries to replay in a single sync message.
	pub batch_size: u64,

	/// The timeout duration for fetching missing log entries during catch-up.
	pub fetch_timeout: Duration,
}

impl Default for Config {
	fn default() -> Self {
		Self {
			batch_size: 2000,
			fetch_timeout: Duration::from_secs(25),
		}
	}
}
