//! State synchronization (catch-up) for followers that are behind the
//! leader's log.
//!
//! When a follower receives a log entry that is not an immediate successor
//! of its last local entry, it knows there is a gap. The [`StateSync`]
//! trait provides the machinery to close that gap — either by replaying
//! the missing log entries from peers ([`LogReplaySync`](super::LogReplaySync))
//! or by transferring a compact snapshot of the state.
//!
//! # Architecture
//!
//! A `StateSync` implementation is a factory for two companion types:
//!
//! - **[`StateSyncProvider`]** — a long-lived object created once per
//!   peer. It runs on *every* node in the group (including the leader)
//!   and serves state to sessions on lagging followers.
//! - **[`StateSyncSession`]** — a short-lived object created on a
//!   lagging follower for the duration of a single catch-up episode.
//!   It drives the sync protocol (requesting data from providers,
//!   applying it to the state machine) and terminates once the
//!   follower's log is fully caught up.
//!
//! Both types communicate via [`StateSync::Message`]s sent through the
//! group's bond network.
//!
//! # Built-in implementation
//!
//! [`LogReplaySync`](super::LogReplaySync) works with any state machine.
//! It recovers missing entries by broadcasting an availability request to
//! all bonded peers, partitioning the needed range across responders, and
//! pulling entries in parallel batches. This is the recommended starting
//! point — switch to a custom implementation only when log replay becomes
//! too slow for your workload.
//!
//! # Custom implementations
//!
//! For high-throughput state machines where replaying every command is
//! expensive, you can implement snapshot-based sync. The general
//! approach is:
//!
//! 1. The **provider** periodically snapshots the state machine and
//!    remembers the log position of the snapshot.
//! 2. When a **session** starts, it requests the latest snapshot from a
//!    provider, loads it into the local state machine, and then replays
//!    only the commands that arrived after the snapshot.
//!
//! The mosaik [`collections`](crate::collections) subsystem uses this
//! strategy internally.

use {
	crate::{
		NetworkId,
		PeerId,
		groups::{Bonds, Cursor, GroupId, Index, StateMachine, Storage, Term},
		primitives::{EncodeError, UniqueId, sealed::Sealed},
	},
	core::task::{Context, Poll},
	serde::{Serialize, de::DeserializeOwned},
};

/// Defines how a lagging follower catches up with the rest of the group.
///
/// A `StateSync` implementation is a **factory** that produces two
/// collaborating types:
///
/// | Type | Lifetime | Runs on | Purpose |
/// |------|----------|---------|---------|
/// | [`StateSyncProvider`] | Group lifetime | Every peer | Serves state to catching-up followers |
/// | [`StateSyncSession`] | Catch-up duration | Lagging follower | Drives one catch-up episode |
///
/// The runtime calls [`create_provider`](Self::create_provider) once at
/// group initialization on every peer, and
/// [`create_session`](Self::create_session) each time a follower detects
/// a gap in its log.
///
/// # Signature and group identity
///
/// Like [`StateMachine::signature`], the
/// [`signature`](Self::signature) of the sync implementation is part
/// of the [`GroupId`](super::GroupId) derivation. All group members
/// must use the same sync implementation with the same configuration,
/// otherwise they will derive different group ids and fail to bond.
///
/// # Using the built-in log replay
///
/// For most use cases [`LogReplaySync`](super::LogReplaySync) is
/// sufficient:
///
/// ```rust,ignore
/// impl StateSync for LogReplaySync<MyMachine> {
///     // All types and methods are already implemented.
/// }
///
/// // In your StateMachine impl:
/// fn state_sync(&self) -> LogReplaySync<Self> {
///     LogReplaySync::default()
///         .with_batch_size(NonZero::new(5000).unwrap())
/// }
/// ```
pub trait StateSync: Send + 'static {
	/// The [`StateMachine`] that this sync implementation is designed
	/// for.
	type Machine: StateMachine;

	/// The wire message type exchanged between providers and sessions
	/// during sync.
	///
	/// This must be serializable because messages are sent over the
	/// group's bond connections.
	type Message: StateSyncMessage;

	/// The provider type — a long-lived object that runs on every peer
	/// and serves state to [`StateSyncSession`] instances created by
	/// lagging followers.
	///
	/// Created once at group initialization via
	/// [`create_provider`](Self::create_provider) and lives for the
	/// entire lifetime of the group.
	type Provider: StateSyncProvider<Owner = Self>;

	/// The session type — a short-lived object created on a lagging
	/// follower for the duration of a single catch-up episode.
	///
	/// Created via [`create_session`](Self::create_session) when a
	/// gap is detected, and dropped once the follower has caught up.
	type Session: StateSyncSession<Owner = Self>;

	/// Returns a unique fingerprint of this sync implementation and
	/// its configuration.
	///
	/// This is part of the [`GroupId`](super::GroupId) derivation —
	/// all group members must return an identical value. Include any
	/// parameters that affect sync behavior and must be uniform
	/// across the group (batch sizes, timeouts, etc.).
	fn signature(&self) -> UniqueId;

	/// Creates the [`StateSyncProvider`] for this peer.
	///
	/// Called exactly **once** per peer at group initialization. The
	/// returned provider will be polled for the entire lifetime of
	/// the group.
	fn create_provider(&self, cx: &dyn SyncContext<Self>) -> Self::Provider;

	/// Creates a new [`StateSyncSession`] on a lagging follower.
	///
	/// This is called the moment a follower receives a log entry that
	/// is not an immediate successor of its last local entry —
	/// indicating a gap that needs to be filled.
	///
	/// # Parameters
	///
	/// - `position` — the leader's log position **preceding** the
	///   arriving entries. This is the point the follower needs to
	///   catch up to.
	/// - `leader_commit` — the leader's latest committed index at the
	///   time the session is created.
	/// - `entries` — the batch of log entries that triggered the
	///   catch-up. The session should buffer these and apply them
	///   once the gap is closed.
	fn create_session(
		&self,
		cx: &mut dyn SyncSessionContext<Self>,
		position: Cursor,
		leader_commit: Index,
		entries: Vec<(Command<Self>, Term)>,
	) -> Self::Session;
}

/// A long-lived server that runs on every peer and serves state to
/// catching-up followers.
///
/// One `StateSyncProvider` instance is created per peer via
/// [`StateSync::create_provider`] at group initialization and lives
/// for the entire group lifetime. Its responsibilities are:
///
/// - **Responding to sync requests** — when a lagging follower's
///   [`StateSyncSession`] sends a message, the provider handles it
///   via [`receive`](Self::receive).
/// - **Background work** — the optional [`poll`](Self::poll) method
///   is called on each tick of the group scheduler, letting the
///   provider drive timeouts, create snapshots, or perform other
///   periodic work.
/// - **Log compaction hints** — via
///   [`safe_to_prune_prefix`](Self::safe_to_prune_prefix), the
///   provider can tell the runtime which log entries are safe to
///   discard.
pub trait StateSyncProvider: Send + 'static {
	/// The parent [`StateSync`] implementation that created this
	/// provider.
	type Owner: StateSync;

	/// Called on each tick of the group's internal work scheduler.
	///
	/// Use this to drive internal timeouts, trigger retries, create
	/// periodic snapshots, or perform any background work that is
	/// independent of incoming messages.
	///
	/// - Return `Poll::Ready(())` to signal that work was completed
	///   and the provider should be polled again immediately.
	/// - Return `Poll::Pending` when idle. Call `cx.waker().wake()`
	///   to request a future poll.
	///
	/// The default implementation returns `Poll::Pending` (no-op).
	fn poll(
		&mut self,
		_: &mut Context<'_>,
		_: &mut dyn SyncProviderContext<Self::Owner>,
	) -> Poll<()> {
		Poll::Pending
	}

	/// Handles an incoming sync message from a remote peer.
	///
	/// Returns `Ok(())` if the message was processed, or
	/// `Err(message)` if the message was not recognized or not
	/// applicable to this provider (e.g. it belongs to a different
	/// protocol version).
	fn receive(
		&mut self,
		message: Message<Self::Owner>,
		sender: PeerId,
		cx: &mut dyn SyncProviderContext<Self::Owner>,
	) -> Result<(), Message<Self::Owner>>;

	/// Returns the highest log index up to which entries can be
	/// safely pruned without breaking in-progress sync sessions.
	///
	/// Returning `None` (the default) means the provider does not
	/// support log pruning — all entries must be retained. This is
	/// the case for [`LogReplaySync`](super::LogReplaySync).
	///
	/// The returned value must never exceed the committed index.
	#[inline]
	fn safe_to_prune_prefix(
		&self,
		_: &mut dyn SyncProviderContext<Self::Owner>,
	) -> Option<Index> {
		None
	}

	/// Called when the group's committed index advances.
	///
	/// Use this to trigger snapshot creation, update internal
	/// bookkeeping, or notify active sync sessions that new data
	/// is available.
	#[inline]
	fn committed(
		&mut self,
		_: Cursor,
		_: &mut dyn SyncProviderContext<Self::Owner>,
	) {
	}
}

/// A short-lived catch-up session running on a single lagging follower.
///
/// Created by [`StateSync::create_session`] when a follower detects a
/// gap in its log, and dropped once the gap is closed. The session
/// drives the sync protocol by:
///
/// 1. Requesting missing state from [`StateSyncProvider`]s on other
///    peers via messages.
/// 2. Receiving responses and applying them to the local state machine
///    (through the [`SyncSessionContext`]).
/// 3. Buffering any new entries that arrive from the leader while the
///    gap is being filled (via [`buffer`](Self::buffer)).
/// 4. Returning `Poll::Ready(position)` from [`poll`](Self::poll)
///    once the follower is fully caught up to the returned position.
pub trait StateSyncSession: Send + 'static {
	/// The parent [`StateSync`] implementation that created this
	/// session.
	type Owner: StateSync;

	/// Drives the sync process forward.
	///
	/// Polled repeatedly by the group scheduler. Return
	/// `Poll::Ready(position)` when the follower is fully
	/// synchronized up to `position`, or `Poll::Pending` to yield
	/// until more data arrives or a timeout fires.
	fn poll(
		&mut self,
		cx: &mut Context<'_>,
		driver: &mut dyn SyncSessionContext<Self::Owner>,
	) -> Poll<Cursor>;

	/// Handles an incoming sync message from a remote provider.
	fn receive(
		&mut self,
		message: Message<Self::Owner>,
		sender: PeerId,
		cx: &mut dyn SyncSessionContext<Self::Owner>,
	);

	/// Buffers new log entries that arrive from the leader while
	/// sync is in progress.
	///
	/// These entries come *after* the gap the session is trying to
	/// fill. The session must retain them and apply them after the
	/// gap is closed.
	fn buffer(
		&mut self,
		position: Cursor,
		entries: Vec<(Command<Self::Owner>, Term)>,
		cx: &mut dyn SyncSessionContext<Self::Owner>,
	);
}

/// Shared context available to both [`StateSyncProvider`] and
/// [`StateSyncSession`] instances.
///
/// Provides access to the group's state machine, log storage, network
/// identity, and messaging primitives. All context methods are
/// determinism-safe — they expose only state that is consistent across
/// nodes.
pub trait SyncContext<S: StateSync>: Sealed {
	/// Returns a shared reference to the state machine.
	fn state_machine(&self) -> &S::Machine;

	/// Returns an exclusive reference to the state machine.
	fn state_machine_mut(&mut self) -> &mut S::Machine;

	/// Returns read-only access to the raw log storage.
	fn log(&self) -> &dyn Storage<Command<S>>;

	/// Returns mutable access to the raw log storage.
	fn log_mut(&mut self) -> &mut dyn Storage<Command<S>>;

	/// The position of the latest committed log entry (replicated to
	/// a quorum).
	fn committed(&self) -> Cursor;

	/// The [`PeerId`] of the local node.
	fn local_id(&self) -> PeerId;

	/// The [`GroupId`] of this group.
	fn group_id(&self) -> GroupId;

	/// The [`NetworkId`] of the network this group belongs to.
	fn network_id(&self) -> NetworkId;

	/// Sends a sync message to a specific peer.
	fn send_to(
		&mut self,
		peer: PeerId,
		message: S::Message,
	) -> Result<(), EncodeError>;

	/// Broadcasts a sync message to all bonded peers, returning the
	/// list of peers the message was sent to.
	fn broadcast(
		&mut self,
		message: S::Message,
	) -> Result<Vec<PeerId>, EncodeError>;

	/// Returns the [`Bonds`] handle for observing connectivity
	/// changes in the group.
	fn bonds(&self) -> Bonds<S::Machine>;
}

/// Extended context for [`StateSyncSession`] instances, available only
/// on the lagging follower during catch-up.
///
/// Adds leader-awareness and the ability to fast-forward the committed
/// index after a snapshot load.
pub trait SyncSessionContext<S: StateSync>: SyncContext<S> {
	/// Returns the [`PeerId`] of the current leader.
	///
	/// Always known when a sync session is active. Useful for
	/// avoiding sync requests to the leader (to prevent overloading
	/// it) or for triggering leader-specific sync logic.
	fn leader(&self) -> PeerId;

	/// Fast-forwards the committed index to the given position.
	///
	/// Used after snapshot-based sync to set the committed index to
	/// the snapshot's anchor position without having the
	/// corresponding log entries.
	fn set_committed(&mut self, position: Cursor);
}

/// Extended context for [`StateSyncProvider`] instances, available on
/// all peers in all Raft roles.
///
/// Adds leader-awareness and the ability to inject commands into the
/// Raft log.
pub trait SyncProviderContext<S: StateSync>: SyncContext<S> {
	/// Returns the [`PeerId`] of the current leader, if known.
	///
	/// May be `None` during elections or network partitions.
	fn leader(&self) -> Option<PeerId>;

	/// Returns `true` if the local node is the current leader.
	#[inline]
	fn is_leader(&self) -> bool {
		self.leader() == Some(self.local_id())
	}

	/// Injects a command into the Raft log (leader only).
	///
	/// This is a **best-effort** operation — the command is not
	/// guaranteed to be committed. It is useful for inserting
	/// sync-protocol markers (e.g. snapshot anchors) that must be
	/// seen by all group members at exactly the same log position.
	///
	/// Returns `Err(command)` if the local node is not the leader.
	fn feed_command(&mut self, command: Command<S>) -> Result<(), Command<S>>;
}

/// Marker trait for sync protocol wire messages.
///
/// Automatically implemented for any type that is `Clone +
/// Serialize + DeserializeOwned + Send + Sync + 'static`.
pub trait StateSyncMessage:
	Clone + Serialize + DeserializeOwned + Send + Sync + 'static
{
}

impl<T> StateSyncMessage for T where
	T: Clone + Serialize + DeserializeOwned + Send + Sync + 'static
{
}

pub type Machine<S> = <S as StateSync>::Machine;
pub type Message<S> = <S as StateSync>::Message;
pub type Command<S> = <Machine<S> as StateMachine>::Command;
pub type Session<S> = <S as StateSync>::Session;
pub type Provider<S> = <S as StateSync>::Provider;
