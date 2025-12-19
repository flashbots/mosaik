#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("unknown group public key in join request")]
	UnknownGroupPublicKey,

	#[error(
		"invalid signature length in join request; expected 64 bytes, got {0} \
		 bytes"
	)]
	InvalidSignatureLength(usize),

	#[error("failed to open link: {0}")]
	OpenLinkFailed(#[from] crate::network::link::OpenError),

	#[error("join request failed: {0}")]
	JoinRequestFailed(#[from] crate::network::link::SendError),

	#[error("failed to receive group state: {0}")]
	ReceiveGroupStateFailed(#[from] crate::network::link::RecvError),
}
