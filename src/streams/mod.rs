use {
	crate::{Discovery, LocalNode, ProtocolProvider, UniqueId},
	iroh::protocol::RouterBuilder,
	listen::Listener,
};

mod config;
mod listen;

pub use config::{Config, ConfigBuilder};

/// A unique identifier for a stream within the Mosaik network.
///
/// By default this id is derived from the stream datum type.
pub type StreamId = UniqueId;

pub struct Streams {
	local: LocalNode,
	discovery: Discovery,
}

impl Streams {
	const ALPN: &'static [u8] = b"/mosaik/streams/1";

	pub fn new(local: LocalNode, discovery: Discovery, config: Config) -> Self {
		Self { local, discovery }
	}
}

impl ProtocolProvider for Streams {
	fn install(&self, protocols: RouterBuilder) -> RouterBuilder {
		protocols.accept(Self::ALPN, Listener)
	}
}
