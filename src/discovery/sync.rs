use {
	super::Catalog,
	crate::LocalNode,
	core::fmt,
	iroh::{
		endpoint::Connection,
		protocol::{AcceptError, ProtocolHandler},
	},
	tokio::sync::watch,
};

#[derive(Clone)]
pub(super) struct CatalogSync {
	local: LocalNode,
	catalog: watch::Receiver<Catalog>,
}

impl CatalogSync {
	pub(super) const ALPN: &'static [u8] = b"/mosaik/discovery/1";

	pub(super) fn new(
		local: LocalNode,
		catalog: watch::Receiver<Catalog>,
	) -> Self {
		Self { local, catalog }
	}
}

impl fmt::Debug for CatalogSync {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("CatalogSync").finish()
	}
}

impl ProtocolHandler for CatalogSync {
	async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
		tracing::info!(
			"Accepting incoming CatalogSync connection from {}",
			connection.remote_id()
		);

		Ok(())
	}
}
