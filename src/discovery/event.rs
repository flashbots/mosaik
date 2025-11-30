use crate::{PeerEntry, PeerId};

/// Discovery system public API events.
#[derive(Debug, Clone)]
pub enum Event {
	/// A new peer has been discovered.
	PeerDiscovered(PeerEntry),

	/// An existing peer has been updated.
	PeerUpdated(PeerEntry),

	/// A peer has broadcasted its departure.
	PeerDeparted(PeerId),

	/// An event from the catalog sync subsystem.
	CatalogSync(super::sync::Event),
}
