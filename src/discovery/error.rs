use crate::PeerId;

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
}
