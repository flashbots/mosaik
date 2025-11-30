use {
	super::{PeerEntry, SignedPeerEntry},
	crate::PeerId,
};

/// A catalog of discovered nodes and their associated peer info.
///
/// Notes:
///
/// - All entries in the catalog are ordered by the `[PeerId`] to maintain
///   consistency across different instances with the same entries.
///
/// - The public API of the catalog allows only read access to the entries;
///   modifications are done internally by the discovery system.
///
/// - The public read API operates at the [`PeerEntry`] level; signed versions
///   of entries are not exposed publicly and are only used internally by the
///   discovery system to verify authenticity of received entries.
///
/// - The catalog is implemented using immutable data structures and each public
///   read access operates on a snapshot of the catalog at a specific point in
///   time.
///
/// - The catalog maintains two separate sets of peer info:
///
///   - Signed peer info, which has been received from other peers and are
///     signed by their private keys. These are considered trustworthy and they
///     are the only entries that are synced to and from other peers.
///
///   - Unsigned peer info, which has been inserted locally (e.g., via test
///     utilities) or for some other reason. Those entries are available on the
///     local node but are not synced to other peers.
///
/// - The catalog is thread-safe and can be accessed concurrently, each access
///   operation works on the most recent snapshot of the catalog.
///
/// - The catalog does not provide a public API for constructing new instances;
///   they are created and owned by the discovery system internally.
#[derive(Debug, Clone)]
pub struct Catalog {
	/// Entries with valid signatures by their authors.
	///
	/// Those entries are synced with other peers.
	pub(super) signed: im::OrdMap<PeerId, SignedPeerEntry>,

	/// Entries without signatures.
	///
	/// Those entries are local only and not synced with other peers.
	pub(super) unsigned: im::OrdMap<PeerId, PeerEntry>,
}

/// Public Read API
impl Catalog {
	/// Returns an iterator over all peer entries in the catalog.
	///
	/// The iterator yields both signed and unsigned entries, with signed entries
	/// being the first to be returned.
	pub fn iter(&self) -> impl DoubleEndedIterator<Item = &PeerEntry> {
		self
			.signed
			.values()
			.map(|signed| signed.as_ref())
			.chain(self.unsigned.values())
	}

	/// Returns a reference to the peer entry for the given peer ID, if it exists.
	///
	/// This method checks both signed and unsigned entries.
	pub fn get(&self, peer_id: &PeerId) -> Option<&PeerEntry> {
		self
			.signed
			.get(peer_id)
			.map(|signed| signed.as_ref())
			.or_else(|| self.unsigned.get(peer_id))
	}

	/// Returns the number of peer entries in the catalog.
	pub fn len(&self) -> usize {
		self.signed.len() + self.unsigned.len()
	}

	/// Returns true if the catalog is empty.
	pub fn is_empty(&self) -> bool {
		self.signed.is_empty() && self.unsigned.is_empty()
	}
}

impl Catalog {
	pub(super) fn new() -> Self {
		Self {
			signed: im::OrdMap::new(),
			unsigned: im::OrdMap::new(),
		}
	}
}
