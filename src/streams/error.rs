use super::Datum;

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("Stream terminated")]
	Terminated,
}

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum PublishError<D: Datum> {
	#[error("stream has no consumers")]
	NoConsumers(D),

	#[error("stream terminated")]
	Terminated,
}
