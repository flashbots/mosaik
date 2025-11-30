use {
	super::Catalog,
	crate::{LocalNode, SignedPeerEntry},
	core::fmt,
	iroh::{
		endpoint::Connection,
		protocol::{AcceptError, ProtocolHandler},
	},
	serde::{Deserialize, Serialize},
	tokio::sync::{
		mpsc::{UnboundedReceiver, unbounded_channel},
		watch,
	},
};

mod event;

pub use event::Event;

/// The catalog synchronization protocol exchanges the current state of the peer
/// catalog between connected peers to ensure they have an up-to-date view of
/// the network.
pub(super) struct CatalogSync {
	local: LocalNode,
	catalog: watch::Sender<Catalog>,
	events: UnboundedReceiver<Event>,
}

impl CatalogSync {
	pub(super) const ALPN: &'static [u8] = b"/mosaik/discovery/sync/1";

	pub(super) fn new(local: LocalNode, catalog: watch::Sender<Catalog>) -> Self {
		let (_, events) = unbounded_channel();
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

	/// Returns the protocol listener instance responsible for accepting incoming
	/// connections for the catalog sync protocol.
	pub const fn protocol(&self) -> &impl ProtocolHandler {
		self
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

/// A snapshot of the discovery catalog at a specific point in time containing
/// all signed peer entries. This is a non-optimal synchronization format used
/// for simplicity in the initial implementation of the catalog sync protocol.
///
/// When peers exchange full catalog snapshots, they can compare the entries and
/// update their local catalogs accordingly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogSnapshot(Vec<SignedPeerEntry>);
