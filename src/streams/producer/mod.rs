mod sender;
mod sink;
mod status;

pub(crate) use sink::FanoutSink;
pub use {
	sender::Producer,
	status::{Status, Subscribed, Unsubscribed},
};

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum PublishError<D: crate::prelude::Datum> {
	#[error("stream has no consumers")]
	NoConsumers(D),

	#[error("stream has been terminated")]
	Terminated,
}
