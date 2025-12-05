use crate::network::{
	PeerId,
	link::{AcceptError, OpenError, RecvError, SendError},
};

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

	#[error("Connection error: {0}")]
	Open(#[from] OpenError),

	#[error("Accept error: {0}")]
	Accept(#[from] AcceptError),

	#[error("I/O error: {0}")]
	Io(#[from] std::io::Error),

	#[error("Recv error: {0}")]
	Recv(#[from] RecvError),

	#[error("Send error: {0}")]
	Send(#[from] SendError),

	#[error("Operation Cancelled")]
	Cancelled,
}
