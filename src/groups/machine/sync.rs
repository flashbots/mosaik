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

/// Provides controlled access for the state synchronization implementation to
/// the internal state of the group protocol.
pub trait StateSyncContext<S: StateSync>: Sealed {
	/// Returns the current state machine instance of the group protocol.
	fn machine(&self) -> &S::Machine;

	/// Returns a mutable reference to the current state machine instance of the
	/// group protocol.
	fn machine_mut(&mut self) -> &mut S::Machine;

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

	/// Returns the `PeerId` of the current leader, if known. This can be used
	/// to deprioritize syncing from the leader to avoid overloading it with sync
	/// requests.
	///
	/// This value is guaranteed to be `Some` when creating a new sync session.
	fn leader(&self) -> Option<PeerId>;

	/// Sends a message to a specific peer.
	fn send_to(&mut self, peer: PeerId, message: S::Message);

	/// Broadcasts a message to all peers in the group.
	fn broadcast(&mut self, message: S::Message);

	/// Returns [`Bonds`] of the group which can be used to observe changes to the
	/// list of connected/bonded peers.
	fn bonds(&self) -> Bonds;
}

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
	/// state to `StateSyncSession` instances during the catch-up process.
	fn create_provider(&self, cx: &dyn StateSyncContext<Self>) -> Self::Provider;

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
		cx: &mut dyn StateSyncContext<Self>,
		position: Cursor,
		leader_commit: Index,
		entries: Vec<(Command<Self>, Term)>,
	) -> Self::Session;
}

pub trait StateSyncProvider: Send + 'static {
	type Owner: StateSync;

	/// Receives a message from a remote peer.
	fn receive(
		&mut self,
		message: Message<Self::Owner>,
		sender: PeerId,
		cx: &mut dyn StateSyncContext<Self::Owner>,
	) -> Result<(), Message<Self::Owner>>;

	/// Returns the log position up to which it is safe to prune log entries
	/// without risking that they are still needed for state synchronization of
	/// lagging followers.
	///
	/// This value should never be greater than the position of the latest
	/// committed log entry.
	fn safe_to_prune_prefix(
		&self,
		_: &mut dyn StateSyncContext<Self::Owner>,
	) -> Option<Index> {
		None
	}

	/// Notifies the provider that the committed index has advanced to a new
	/// position. This can be used by the provider to update its internal state,
	/// create snapshots, etc.
	fn committed(&mut self, _: Index, _: &mut dyn StateSyncContext<Self::Owner>) {
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
	fn poll_next_tick(
		&mut self,
		cx: &mut Context<'_>,
		driver: &mut dyn StateSyncContext<Self::Owner>,
	) -> Poll<Cursor>;

	/// Receives a message from a remote peer.
	fn receive(
		&mut self,
		message: Message<Self::Owner>,
		sender: PeerId,
		cx: &mut dyn StateSyncContext<Self::Owner>,
	);

	/// Accepts a batch of log entries that have been produced by the current
	/// leader while the sync process is ongoing. Those entries are going to be
	/// immediately after the gap we're trying to fill.
	fn buffer(
		&mut self,
		position: Cursor,
		entries: Vec<(Command<Self::Owner>, Term)>,
		cx: &mut dyn StateSyncContext<Self::Owner>,
	);
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
