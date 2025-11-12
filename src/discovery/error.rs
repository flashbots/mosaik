#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("Gossip topic join error: {0}")]
	GossipTopicJoin(#[from] iroh_gossip::api::ApiError),

	#[error("Dial error: {0}")]
	Dial(#[from] iroh::endpoint::ConnectError),

	#[error("Connection error: {0}")]
	Connection(#[from] iroh::endpoint::ConnectionError),

	#[error("Connection dropped")]
	ConnectionDropped,

	#[error("Invalid handshake request: {0}")]
	InvalidHandshakeRequest(#[from] rmp_serde::decode::Error),

	#[error("I/O error: {0}")]
	Io(#[from] std::io::Error),

	#[error("Event loop terminated")]
	Terminated,
}
