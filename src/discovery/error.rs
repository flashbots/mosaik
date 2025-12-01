use {
	crate::PeerId,
	iroh::endpoint::{ConnectError, ConnectionError},
	n0_error::Meta,
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
	Connection(#[from] ConnectError),

	#[error("I/O error: {0}")]
	Io(#[from] std::io::Error),

	#[error("Serialization error: {0}")]
	Decode(#[from] bincode::error::DecodeError),
}

impl From<ConnectionError> for Error {
	#[track_caller]
	fn from(err: ConnectionError) -> Self {
		Error::Connection(ConnectError::Connection {
			source: err,
			meta: Meta::default(),
		})
	}
}
