mod sender;
mod sink;
mod status;

pub use sender::Producer;
pub(crate) use {sink::FanoutSink, status::Status};

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum PublishError<D: crate::prelude::Datum> {
	#[error("stream has no consumers")]
	NoConsumers(D),

	#[error("stream has been terminated")]
	Terminated,
}
