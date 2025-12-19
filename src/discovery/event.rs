use {
	super::PeerEntry,
	crate::{network::PeerId, primitives::Short},
	core::fmt,
	derive_more::PartialEq,
};

/// Discovery system public API events.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
	/// A new peer has been discovered.
	PeerDiscovered(PeerEntry),

	/// An existing peer has been updated.
	PeerUpdated(PeerEntry),

	/// A peer has broadcasted its departure.
	PeerDeparted(PeerId),
}

impl fmt::Display for Short<&Event> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match &self.0 {
			Event::PeerDiscovered(entry) => {
				write!(f, "Event::PeerDiscovered({})", Short(entry))
			}
			Event::PeerUpdated(entry) => {
				write!(f, "Event::PeerUpdated({})", Short(entry))
			}
			Event::PeerDeparted(peer_id) => {
				write!(f, "Event::PeerDeparted({})", Short(peer_id))
			}
		}
	}
}
