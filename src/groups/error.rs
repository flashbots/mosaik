use crate::{
	Groups,
	discovery::Error as DiscoveryError,
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
