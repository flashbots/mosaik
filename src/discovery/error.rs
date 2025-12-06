use crate::network::{PeerId, link::LinkError};

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("PeerId does not match the secret key, expected {0}, got {1}")]
	InvalidSecretKey(PeerId, PeerId),

	#[error("Signature is invalid")]
	InvalidSignature,

	#[error("Failed to join gossip topic: {0}")]
	GossipJoin(#[from] iroh_gossip::api::ApiError),

	#[error("Invalid local PeerEntry update attempted: was {0}, attempted {1}")]
	PeerIdChanged(PeerId, PeerId),

	#[error("Link error: {0}")]
	Link(#[from] LinkError),

	#[error("Other error: {0}")]
	Other(#[from] Box<dyn std::error::Error + Send + Sync>),

	#[error("Operation Cancelled")]
	Cancelled,
}
