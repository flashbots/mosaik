use {
	crate::{LocalNode, PeerId, ProtocolProvider},
	iroh::protocol::RouterBuilder,
	iroh_gossip::Gossip,
	sync::CatalogSync,
};

mod catalog;
mod sync;

pub use catalog::Catalog;

/// The discovery system for a Mosaik network.
///
/// The discovery system is composed of two main protocols:
///
/// - A gossip protocol that peers use to broadcast their presence and changes
///   to their own state and metadata.
///
/// - A catalog synchronization protocol that peers use to exchange and
///   synchronize their catalogs of known peers and their associated metadata.
pub struct Discovery {
	local: LocalNode,
	gossip: Gossip,
	catalog: Catalog,
	sync: CatalogSync,
}

/// Public API
impl Discovery {
	pub fn dial(&self, peer: PeerId) {
		todo!()
	}
}

/// Internal API
impl Discovery {
	const ALPN_GOSSIP: &'static [u8] = b"/mosaik/gossip/1";

	pub(crate) fn new(
		local: LocalNode,
		bootstrap: impl IntoIterator<Item = PeerId>,
	) -> Self {
		let gossip = Gossip::builder()
			.alpn(Self::ALPN_GOSSIP)
			.spawn(local.endpoint().clone());

		let catalog = Catalog;
		let sync = CatalogSync::new(local.clone(), catalog.clone());

		let instance = Self {
			local,
			gossip,
			catalog,
			sync,
		};

		for peer in bootstrap {
			instance.dial(peer);
		}
		instance
	}
}

impl ProtocolProvider for Discovery {
	fn install(&self, protocols: RouterBuilder) -> RouterBuilder {
		protocols
			.accept(Self::ALPN_GOSSIP, self.gossip.clone())
			.accept(CatalogSync::ALPN, self.sync.clone())
	}
}
