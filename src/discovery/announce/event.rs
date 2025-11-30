use crate::PeerEntry;

#[derive(Debug, Clone)]
pub enum Event {
	/// A new peer has been discovered.
	PeerDiscovered(PeerEntry),

	/// An existing peer has been updated.
	PeerUpdated(PeerEntry),

	/// A peer has been lost.
	PeerLost(PeerEntry),
}
