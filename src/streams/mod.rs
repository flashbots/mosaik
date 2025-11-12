use {
	crate::local::Local,
	derive_more::Display,
	iroh::protocol::RouterBuilder,
	protocol::Protocol,
	serde::{Deserialize, Serialize, de::DeserializeOwned},
};

mod channel;
mod consumer;
mod criteria;
mod error;
mod fanout;
mod producer;
mod protocol;

pub(crate) use fanout::FanoutSink;
pub use {
	consumer::Consumer,
	criteria::Criteria,
	error::Error,
	producer::Producer,
};

/// This type uniquely identifies a stream within the network.
///
/// Notes:
/// - At present, stream ids are derived from the data rust type name.
#[derive(
	Debug,
	Clone,
	Display,
	Serialize,
	Deserialize,
	PartialEq,
	Eq,
	PartialOrd,
	Ord,
	Hash,
)]
pub struct StreamId(String);

impl StreamId {
	pub fn of<D: Datum>() -> Self {
		Self(core::any::type_name::<D>().to_string())
	}
}

/// Implemented by all data types that are published to streams.
pub trait Datum:
	core::fmt::Debug
	+ PartialEq
	+ Serialize
	+ DeserializeOwned
	+ Send
	+ Sync
	+ 'static
{
	fn key(&self) -> &str {
		core::any::type_name_of_val(&self)
	}
}

impl<T> Datum for T where
	T: core::fmt::Debug
		+ PartialEq
		+ Serialize
		+ DeserializeOwned
		+ Send
		+ Sync
		+ 'static
{
}

pub struct Streams {
	local: Local,
}

impl Streams {
	const ALPN_STREAMS: &'static [u8] = b"/mosaik/streams/1";

	pub fn new(local: Local) -> Self {
		Self { local }
	}

	pub fn attach(&mut self, router: RouterBuilder) -> RouterBuilder {
		router.accept(Self::ALPN_STREAMS, Protocol::new(self.local.clone()))
	}
}
