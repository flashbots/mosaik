use crate::streams::StreamId;

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("Stream terminated")]
	Terminated,

	#[error("Mismatched stream ID: expected {expected}, found {found}")]
	MismatchedStreamId { expected: StreamId, found: StreamId },

	#[error("I/O error: {0}")]
	Io(#[from] std::io::Error),

	#[error("Connection error: {0}")]
	Connect(#[from] iroh::endpoint::ConnectError),

	#[error("Connection error: {0}")]
	Connection(#[from] iroh::endpoint::ConnectionError),

	#[error("Remote ID error: {0}")]
	RemoteId(#[from] iroh::endpoint::RemoteEndpointIdError),

	#[error("Protocol error: {0}")]
	Protocol(#[from] ProtocolError),
}

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
	#[error("Connection closed before handshake")]
	ClosedBeforeHandshake,

	#[error("Invalid handshake request: {0}")]
	InvalidHandshakeRequest(rmp_serde::decode::Error),
}
