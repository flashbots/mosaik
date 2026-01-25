use crate::network::link::LinkError;

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("Invalid group key: {0:?}")]
	InvalidGroupKey(super::key::Error),

	#[error("Link error: {0}")]
	LinkError(#[from] LinkError),
}
