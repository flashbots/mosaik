use crate::SignedPeerEntry;

#[derive(Debug, Clone)]
pub enum Event {
	/// A valid and signed peer entry has been updated.
	PeerEntryReceived(SignedPeerEntry),
}
