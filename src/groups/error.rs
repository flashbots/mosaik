use crate::{
	Groups,
	network::link::{Link, LinkError},
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("Group has been terminated")]
	GroupTerminated,

	#[error("Invalid group key: {0:?}")]
	InvalidGroupKey(super::key::Error),

	#[error("Link error: {0}")]
	LinkError(#[from] LinkError),

	#[error("An active bond already exists with this peer")]
	AlreadyBonded(Link<Groups>),
}
