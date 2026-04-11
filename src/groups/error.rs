use {
	crate::{
		discovery::Error as DiscoveryError,
		groups::{Groups, StateMachine},
		network::{
			self,
			link::{Link, LinkError},
		},
		primitives::EncodeError,
	},
	core::fmt::Debug,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("Group has been terminated")]
	GroupTerminated,

	#[error("Invalid group secret key proof provided")]
	InvalidGroupKeyProof,

	#[error("Unauthorized")]
	Unauthorized,

	#[error("Link error: {0}")]
	Link(#[from] LinkError),

	#[error("An active bond already exists with this peer")]
	AlreadyBonded(Box<Link<Groups>>),

	#[error("Discovery error: {0}")]
	Discovery(DiscoveryError),
}

/// Errors that are communicated by the public API when issuing commands to the
/// group.
#[derive(thiserror::Error)]
pub enum CommandError<M: StateMachine> {
	/// This is a temporary error indicating that the local node is currently
	/// offline and cannot process the command. The error carries the unsent
	/// commands, which can be retried later when the node is back online.
	///
	/// See [`When::is_online`] for more details on how the online/offline status
	/// of the node is determined.
	#[error("Group is temporarily offline and cannot process commands")]
	Offline(Vec<M::Command>),

	#[error("No commands provided")]
	NoCommands,

	/// The group is permanently terminated and cannot process any more commands.
	/// This error is unrecoverable and indicates the group worker loop is no
	/// longer running.
	#[error("Group is terminated")]
	GroupTerminated,

	/// The provided commands could not be encoded for sending over the network.
	#[error("Command encoding error: {1}")]
	Encoding(Vec<M::Command>, EncodeError),
}

impl<M: StateMachine> Debug for CommandError<M> {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		match self {
			Self::Offline(commands) => f
				.debug_tuple("CommandError::Offline")
				.field(&format!("{} commands", commands.len()))
				.finish(),
			Self::NoCommands => f.debug_tuple("CommandError::NoCommands").finish(),
			Self::GroupTerminated => {
				f.debug_tuple("CommandError::GroupTerminated").finish()
			}
			Self::Encoding(commands, err) => f
				.debug_tuple("CommandError::Encoding")
				.field(&format!("{} commands", commands.len()))
				.field(err)
				.finish(),
		}
	}
}

/// Errors that are communicated by the public API when issuing queries to the
/// group.
#[derive(thiserror::Error)]
pub enum QueryError<M: StateMachine> {
	/// This is a temporary error indicating that the local node is currently
	/// offline and cannot process the query. The error carries the unsent query,
	/// which can be retried later when the node is back online.
	///
	/// See [`When::is_online`] for more details on how the online/offline status
	/// of the node is determined.
	#[error("Group is temporarily offline and cannot process queries")]
	Offline(M::Query),

	/// The group is permanently terminated and cannot process any more queries.
	/// This error is unrecoverable and indicates the group worker loop is no
	/// longer running.
	#[error("Group is terminated")]
	GroupTerminated,

	/// The provided query could not be encoded for sending over the network.
	#[error("Query encoding error: {1}")]
	Encoding(M::Query, EncodeError),
}

impl<M: StateMachine> Debug for QueryError<M> {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		match self {
			Self::Offline(_) => {
				f.debug_tuple("QueryError::Offline").finish_non_exhaustive()
			}
			Self::GroupTerminated => {
				f.debug_tuple("QueryError::GroupTerminated").finish()
			}
			Self::Encoding(_, err) => f
				.debug_tuple("QueryError::Encoding")
				.field(err)
				.finish_non_exhaustive(),
		}
	}
}

network::make_close_reason!(
	/// An error occurred during the handshake receive or decode process.
	pub struct InvalidHandshake, 30_400);

network::make_close_reason!(
	/// The peer is not allowed to join the group because it does not
	/// have a valid authentication ticket.
	pub struct NotAllowed, 30_403);

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
