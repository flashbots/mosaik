use {
	super::Catalog,
	crate::LocalNode,
	core::fmt,
	iroh::{
		endpoint::Connection,
		protocol::{AcceptError, ProtocolHandler},
	},
};

#[derive(Clone)]
pub(super) struct CatalogSync {
	catalog: Catalog,
	local: LocalNode,
}

impl CatalogSync {
	pub(super) const ALPN: &'static [u8] = b"/mosaik/discovery/1";

	pub(super) fn new(local: LocalNode, catalog: Catalog) -> Self {
		Self { catalog, local }
	}
}

impl fmt::Debug for CatalogSync {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("CatalogSync").finish()
	}
}

impl ProtocolHandler for CatalogSync {
	async fn accept(&self, _connection: Connection) -> Result<(), AcceptError> {
		todo!()
	}
}
