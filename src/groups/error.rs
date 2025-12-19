#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("unknown group public key in join request")]
	UnknownGroupPublicKey,

	#[error(
		"invalid signature length in join request; expected 64 bytes, got {0} \
		 bytes"
	)]
	InvalidSignatureLength(usize),
}
