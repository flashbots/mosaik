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

	#[error("Invalid message: {0}")]
	InvalidMessage(#[from] rmp_serde::decode::Error),

	#[error("I/O error: {0}")]
	Io(#[from] std::io::Error),

	#[error("Event loop terminated")]
	Terminated,

	#[error("Write error: {0}")]
	Write(#[from] iroh::endpoint::WriteError),

	#[error("Read to end error: {0}")]
	ReadToEnd(#[from] iroh::endpoint::ReadToEndError),

	#[error("Signature validation failed for signed peer info")]
	InvalidSignedPeerInfo,

	#[error("Failed to send command: {0}")]
	SendCommand(
		#[from] tokio::sync::mpsc::error::SendError<crate::discovery::Command>,
	),

	#[error("Received no data when comparing catalog hashes")]
	EmptyCatalogHashCompareResponse,

	#[error("Expected `CatalogHashCompareResponse`, but did not receive it")]
	InvalidCatalogHashCompareResponse,
}

impl Error {
	pub fn dial(err: iroh::endpoint::ConnectError) -> Self {
		Error::Dial(err)
	}

	pub fn connection(err: iroh::endpoint::ConnectionError) -> Self {
		Error::Connection(err)
	}

	pub fn invalid_message(err: rmp_serde::decode::Error) -> Self {
		Error::InvalidMessage(err)
	}

	pub fn write(err: iroh::endpoint::WriteError) -> Self {
		Error::Write(err)
	}

	pub fn read_to_end(err: iroh::endpoint::ReadToEndError) -> Self {
		Error::ReadToEnd(err)
	}

	pub fn send_command(
		err: tokio::sync::mpsc::error::SendError<crate::discovery::Command>,
	) -> Self {
		Error::SendCommand(err)
	}
}
