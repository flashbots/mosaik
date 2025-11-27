use {
	crate::{Discovery, LocalNode, ProtocolProvider},
	accept::Acceptor,
	iroh::protocol::RouterBuilder,
};

mod accept;

pub struct Streams {}

impl Streams {
	const ALPN: &'static [u8] = b"/mosaik/streams/1";

	pub fn new(_local: LocalNode, _discovery: &Discovery) -> Self {
		Self {}
	}
}

impl ProtocolProvider for Streams {
	fn install(&self, protocols: RouterBuilder) -> RouterBuilder {
		protocols.accept(Self::ALPN, Acceptor)
	}
}
