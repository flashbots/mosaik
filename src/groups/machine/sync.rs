//! State synchronization (catch-up) implementation for followers that are not
//! up-to-date with the current state of the group.

use {
	crate::{
		NetworkId,
		PeerId,
		groups::{Bonds, Cursor, GroupId, Index, StateMachine, Storage, Term},
		primitives::{UniqueId, sealed::Sealed},
	},
	core::task::{Context, Poll},
	serde::{Serialize, de::DeserializeOwned},
};

/// Implements the state synchronization (catch-up) process for followers that
/// are not up-to-date with the current state of the group.
pub trait StateSync: Send + 'static {
	/// The state machine type for which this state synchronization implementation
	/// is designed.
	type Machine: StateMachine;

	/// The implementation-specific protocol message that are exchanged between
	/// peers on the wire during the state synchronization process.
	type Message: StateSyncMessage;

	/// Instances of this type run on every peer in the group and are responsible
	/// for serving state to `StateSyncSession` instances that are created by
	/// lagging followers during the catch-up process.
	///
	/// Those are long-lived objects that are created at group initialization and
	/// run for the entire lifetime of the group.
	type Provider: StateSyncProvider<Owner = Self>;

	/// Instances of this type are created by the lagging follower for the
	/// duration of the catch-up process and terminated once the follower is
	/// fully synchronized with the current state of the group.
	type Session: StateSyncSession<Owner = Self>;

	/// Returns a unique identifier for this state synchronization implementation
	/// and its settings. This value is part of the group id derivation and must
	/// be identical for all members of the same group. Any difference in this
	/// value will render a different group id and will prevent peers from joining
	/// the same group.
	///
	/// Examples of relevant settings that should alter the signature are things
	/// like batch sizes, timeouts, or any other parameters that should be
	/// identical across all members of the same group.
	fn signature(&self) -> UniqueId;

	/// Returns a new instance of the state synchronization provider that serves
	/// state to [`StateSyncSession`] instances during the catch-up process.
	///
	/// This method is called exactly once on every peer in the group at
	/// initialization.
	fn create_provider(&self, cx: &dyn SyncContext<Self>) -> Self::Provider;

	/// Creates a new state synchronization session for a specific lagging
	/// follower that is undergoing the catch-up process.
	///
	/// This is called the moment a follower receives the first log entry that is
	/// not an immediate successor of the last local entry, which indicates that
	/// the follower is behind and needs to start the catch-up process.
	///
	/// The `position` parameter indicates the log position at the leader
	/// preceding the arriving entries, which is the position to which the
	/// follower needs to catch up. The `entries` parameter contains the batch of
	/// log entries that triggered the catch-up process, which should be buffered
	/// by the session and applied to the state machine once the follower has
	/// caught up with the leader.
	///
	/// The `leader_commit` parameter indicates the latest committed log index
	/// at the leader, at the moment the sync session is created.
	fn create_session(
		&self,
		cx: &mut dyn SyncSessionContext<Self>,
		position: Cursor,
		leader_commit: Index,
		entries: Vec<(Command<Self>, Term)>,
	) -> Self::Session;
}

pub trait StateSyncProvider: Send + 'static {
	type Owner: StateSync;

	/// Polls the provider on every tick of the group-internal work scheduler.
	/// This can be used to drive internal timeouts, trigger retries, etc.
	///
	/// When this method returns `Poll::Ready(())`, the provider is indicating
	/// that it has completed some work and that it is ready to be polled again
	/// immediately to drive the next step of the state sync process.
	///
	/// The provider can also wake the scheduler at any time by calling
	/// `cx.wake()`, which will cause the scheduler to poll the provider
	/// again.
	///
	/// This is an optional method that is usable by sync providers that need to
	/// do background work irrespective of receiving messages from followers.
	fn poll(
		&mut self,
		_: &mut Context<'_>,
		_: &mut dyn SyncProviderContext<Self::Owner>,
	) -> Poll<()> {
		Poll::Pending
	}

	/// Receives a message from a remote peer.
	fn receive(
		&mut self,
		message: Message<Self::Owner>,
		sender: PeerId,
		cx: &mut dyn SyncProviderContext<Self::Owner>,
	) -> Result<(), Message<Self::Owner>>;

	/// Returns the log position up to which it is safe to prune log entries
	/// without risking that they are still needed for state synchronization of
	/// lagging followers.
	///
	/// This value should never be greater than the position of the latest
	/// committed log entry.
	fn safe_to_prune_prefix(
		&self,
		_: &mut dyn SyncProviderContext<Self::Owner>,
	) -> Option<Index> {
		None
	}

	/// Notifies the provider that the committed index has advanced to a new
	/// position. This can be used by the provider to update its internal state,
	/// create snapshots, etc.
	fn committed(
		&mut self,
		_: Index,
		_: &mut dyn SyncProviderContext<Self::Owner>,
	) {
	}
}

/// Represents one active state synchronization process for a specific follower
/// that is not up-to-date.
///
/// Instances of this type are created by the lagging follower for the duration
/// of the catch-up process and terminated once the follower is fully
/// synchronized with the current state of the group.
pub trait StateSyncSession: Send + 'static {
	type Owner: StateSync;

	/// Drives the state synchronization process forward by polling for the next
	/// tick according to the group-internal work scheduler. This method will be
	/// polled repeatedly until it returns `Poll::Ready(position)`, at which
	/// point the state synchronization process is complete and the follower is
	/// fully synchronized with the current state of the group up to the returned
	/// position.
	fn poll(
		&mut self,
		cx: &mut Context<'_>,
		driver: &mut dyn SyncSessionContext<Self::Owner>,
	) -> Poll<Cursor>;

	/// Receives a message from a remote peer.
	fn receive(
		&mut self,
		message: Message<Self::Owner>,
		sender: PeerId,
		cx: &mut dyn SyncSessionContext<Self::Owner>,
	);

	/// Accepts a batch of log entries that have been produced by the current
	/// leader while the sync process is ongoing. Those entries are going to be
	/// immediately after the gap we're trying to fill.
	fn buffer(
		&mut self,
		position: Cursor,
		entries: Vec<(Command<Self::Owner>, Term)>,
		cx: &mut dyn SyncSessionContext<Self::Owner>,
	);
}

/// Context object that is passed to instances of [`StateSyncProvider`] and
/// [`StateSyncSession`] instances to provide them with access to the state of
/// the group.
pub trait SyncContext<S: StateSync>: Sealed {
	/// Returns the current state machine instance of this group.
	fn state_machine(&self) -> &S::Machine;

	/// Returns a mutable reference to the current state machine instance of this
	/// group.
	fn state_machine_mut(&mut self) -> &mut S::Machine;

	/// Returns a read-only access to the raw log entries storage.
	fn log(&self) -> &dyn Storage<Command<S>>;

	/// Returns a mutable access to the raw log entries storage.
	fn log_mut(&mut self) -> &mut dyn Storage<Command<S>>;

	/// Returns the index of the latest committed log entry that has received a
	/// quorum of votes.
	fn committed(&self) -> Index;

	/// Returns the `PeerId` of the local node.
	fn local_id(&self) -> PeerId;

	/// Returns the unique identifier of the group.
	fn group_id(&self) -> GroupId;

	/// Returns the unique identifier of the network.
	fn network_id(&self) -> NetworkId;

	/// Sends a message to a specific peer.
	fn send_to(&mut self, peer: PeerId, message: S::Message);

	/// Broadcasts a message to all peers in the group and returns the list of
	/// peers the message was sent to.
	fn broadcast(&mut self, message: S::Message) -> Vec<PeerId>;

	/// Returns [`Bonds`] of the group which can be used to observe changes to the
	/// list of connected/bonded peers.
	fn bonds(&self) -> Bonds;
}

/// Context object that is passed to instances of [`StateSyncSession`] instances
/// to provide them with access to the state of the group.
///
/// Instances of this type are instantiated on lagging followers during
/// state catch-up.
pub trait SyncSessionContext<S: StateSync>: SyncContext<S> {
	/// Returns the `PeerId` of the current leader.
	///
	/// When a sync session is triggered, the leader is always known. This can be
	/// used to deprioritize syncing from the leader to avoid overloading it with
	/// sync requests, or to trigger specific logic that can only be executed on
	/// the leader during the sync process.
	fn leader(&self) -> PeerId;
}

/// Context object that is passed to instances of [`StateSyncProvider`]
/// instances to provide them with access to the state of the group.
///
/// Instances of this type are instantiated on all peers in the group running
/// in all roles.
pub trait SyncProviderContext<S: StateSync>: SyncContext<S> {
	/// Returns the `PeerId` of the current leader, if known.
	///
	/// The leader is not guaranteed to be known at all times on all peers as the
	/// network topology changes.
	fn leader(&self) -> Option<PeerId>;

	/// Returns `true` if the local node is the current leader, `false` otherwise.
	fn is_leader(&self) -> bool {
		self.leader() == Some(self.local_id())
	}

	/// Adds a new command to the log of the group. This call is only valid on the
	/// current leader and will be rejected if called on a non-leader node.
	///
	/// This is a best-effort method that can be used by the provider to feed new
	/// commands into the log during the sync process, for example to create new
	/// markers seen by all group members at the same position in the log, there
	/// is no guarantee of timing, delivery or order of the command added through
	/// this method. The only guarantee is that if this command makes it to the
	/// state machine, all nodes in the network will see it at exactly the same
	/// position.
	fn feed_command(&mut self, command: Command<S>) -> Result<(), Command<S>>;
}

pub trait StateSyncMessage:
	Clone + Serialize + DeserializeOwned + Send + 'static
{
}

impl<T> StateSyncMessage for T where
	T: Clone + Serialize + DeserializeOwned + Send + 'static
{
}

pub type Machine<S> = <S as StateSync>::Machine;
pub type Message<S> = <S as StateSync>::Message;
pub type Command<S> = <Machine<S> as StateMachine>::Command;
pub type Session<S> = <S as StateSync>::Session;
pub type Provider<S> = <S as StateSync>::Provider;
