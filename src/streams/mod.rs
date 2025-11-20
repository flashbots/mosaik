mod consumer;
mod datum;
mod error;
mod link;
mod producer;
mod protocol;

pub(crate) use producer::FanoutSink;
pub use {
	consumer::Consumer,
	datum::{Criteria, Datum, StreamId},
	error::Error,
	producer::Producer,
};

pub struct Streams {
	local: crate::local::Local,
}

impl Streams {
	const ALPN_STREAMS: &'static [u8] = b"/mosaik/streams/1";

	pub fn new(local: crate::local::Local) -> Self {
		Self { local }
	}

	pub fn attach(
		&mut self,
		router: iroh::protocol::RouterBuilder,
	) -> iroh::protocol::RouterBuilder {
		router.accept(
			Self::ALPN_STREAMS,
			protocol::Protocol::new(self.local.clone()),
		)
	}
}
