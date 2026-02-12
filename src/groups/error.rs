use crate::{
	Groups,
	discovery::Error as DiscoveryError,
	groups::StateMachine,
	network::{
		self,
		link::{Link, LinkError},
	},
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("Group has been terminated")]
	GroupTerminated,

	#[error("Invalid group secret key proof provided")]
	InvalidGroupKeyProof,

	#[error("Link error: {0}")]
	Link(#[from] LinkError),

	#[error("An active bond already exists with this peer")]
	AlreadyBonded(Box<Link<Groups>>),

	#[error("Discovery error: {0}")]
	Discovery(DiscoveryError),
}

/// Errors that are communicated by the public API when issuing commands to the
/// group.
#[derive(Debug, thiserror::Error)]
pub enum CommandError<M: StateMachine> {
	/// This is a temporary error indicating that the local node is currently
	/// offline and cannot process the command. The error carries the unsent
	/// commands, which can be retried later when the node is back online.
	///
	/// See [`When::is_online`] for more details on how the online/offline status
	/// of the node is determined.
	#[error("Group is temporarily offline and cannot process commands")]
	Offline(Vec<M::Command>),

	/// The group is permanently terminated and cannot process any more commands.
	/// This error is unrecoverable and indicates the group worker loop is no
	/// longer running.
	#[error("Group is terminated")]
	GroupTerminated,
}

/// Errors that are communicated by the public API when issuing queries to the
/// group.
#[derive(Debug, thiserror::Error)]
pub enum QueryError<M: StateMachine> {
	/// This is a temporary error indicating that the local node is currently
	/// offline and cannot process the query. The error carries the unsent query,
	/// which can be retried later when the node is back online.
	///
	/// See [`When::is_online`] for more details on how the online/offline status
	/// of the node is determined.
	#[error("Group is temporarily offline and cannot process queries")]
	Offline(M::Query),

	/// The group is permanently terminated and cannot process any more commands.
	/// This error is unrecoverable and indicates the group worker loop is no
	/// longer running.
	#[error("Group is terminated")]
	GroupTerminated,
}

network::make_close_reason!(
	/// An error occurred during the handshake receive or decode process.
	pub struct InvalidHandshake, 30_400);

network::make_close_reason!(
	/// The group id specified in the handshake is not known to the accepting node.
	pub struct GroupNotFound, 30_404);

network::make_close_reason!(
	/// The authentication proof provided in the handshake is invalid.
	pub struct InvalidProof, 30_405);

network::make_close_reason!(
	/// Timed out while waiting for a response from the remote peer.
	pub struct Timeout, 30_408);

network::make_close_reason!(
	/// A link between those two peers in the same group already exists.
	pub struct AlreadyBonded, 30_429);
