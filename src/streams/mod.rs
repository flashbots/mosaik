mod consumer;
mod error;
mod ext;
mod link;
mod producer;
mod protocol;

pub(crate) use producer::FanoutSink;
pub use {
	consumer::{Consumer, Error as ConsumerError, Status as ConsumerStatus},
	error::Error,
	ext::*,
	producer::Producer,
};

pub(crate) struct Streams {
	local: crate::local::Local,
}

impl Streams {
	const ALPN_STREAMS: &'static [u8] = b"/mosaik/streams/1";

	pub(crate) fn new(local: crate::local::Local) -> Self {
		Self { local }
	}

	pub(crate) fn attach(
		&mut self,
		router: iroh::protocol::RouterBuilder,
	) -> iroh::protocol::RouterBuilder {
		router.accept(
			Self::ALPN_STREAMS,
			protocol::Protocol::new(self.local.clone()),
		)
	}
}
