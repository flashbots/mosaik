use {
	super::Catalog,
	crate::LocalNode,
	core::fmt,
	iroh::{
		endpoint::Connection,
		protocol::{AcceptError, ProtocolHandler},
	},
	tokio::sync::{mpsc::UnboundedReceiver, watch},
};

mod driver;
mod event;

pub use event::Event;

pub(super) struct CatalogSync {
	local: LocalNode,
	catalog: watch::Receiver<Catalog>,
	events: UnboundedReceiver<Event>,
}

impl CatalogSync {
	pub(super) const ALPN: &'static [u8] = b"/mosaik/discovery/sync/1";

	pub(super) fn new(
		local: LocalNode,
		catalog: watch::Receiver<Catalog>,
	) -> Self {
		let (_, events) = tokio::sync::mpsc::unbounded_channel();
		Self {
			local,
			catalog,
			events,
		}
	}

	/// Returns a mutable reference to the events receiver.
	///
	/// This is polled by the discovery worker to process incoming events from the
	/// announcement protocol.
	pub(super) const fn events(&mut self) -> &mut UnboundedReceiver<Event> {
		&mut self.events
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
