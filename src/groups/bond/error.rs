use crate::{discovery::Error as DiscoveryError, network::link::LinkError};

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("Link error: {0}")]
	Link(LinkError),

	#[error("Discovery error: {0}")]
	Discovery(DiscoveryError),

	#[error("Invalid authentication during bond handshake")]
	InvalidProof,
}
