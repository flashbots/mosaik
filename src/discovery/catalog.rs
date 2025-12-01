use {
	super::{Config, Event, PeerEntry, SignedPeerEntry},
	crate::{LocalNode, PeerId},
};

/// A catalog of discovered nodes and their associated peer info.
///
/// Notes:
///
/// - All entries in the catalog are ordered by the [`PeerId`] to maintain
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
///
/// - The catalog is the authoritative source of truth about the local node's
///   own peer entry. The local node's peer entry is always present in the
///   catalog and can be accessed via the public API.
///
/// - The local node's peer entry is always signed. Inserting an unsigned local
///   peer entry is not allowed.
///
/// - Signed entries have precedence over unsigned entries when a signed entry
///   is inserted into the catalog for a peer that already has an unsigned entry
///   with the same peer ID then the unsigned entry is removed.
#[derive(Debug, Clone)]
pub struct Catalog {
	/// The local node's peer ID.
	///
	/// This is used to exclude the local node from queries.
	local_id: PeerId,

	/// Entries with valid signatures by their authors.
	///
	/// Those entries are synced with other peers.
	signed: im::OrdMap<PeerId, SignedPeerEntry>,

	/// Entries without signatures.
	///
	/// Those entries are local only and not synced with other peers.
	unsigned: im::OrdMap<PeerId, PeerEntry>,
}

/// Public Read API
impl Catalog {
	/// Returns an iterator over all peer entries in the catalog excluding the
	/// local peer entry.
	///
	/// The iterator yields both signed and unsigned entries, with signed entries
	/// being the first to be returned.
	pub fn peers(&self) -> impl DoubleEndedIterator<Item = &PeerEntry> {
		self
			.signed
			.values()
			.map(|signed| signed.as_ref())
			.chain(self.unsigned.values())
			.filter(|p| *p.id() != self.local_id)
	}

	/// Returns an iterator over all peer entries that carry a valid signature in
	/// the catalog excluding the local peer entry.
	pub fn signed_peers(&self) -> impl DoubleEndedIterator<Item = &PeerEntry> {
		self
			.signed
			.values()
			.map(|signed| signed.as_ref())
			.filter(|p: &&PeerEntry| *p.id() != self.local_id)
	}

	/// Returns an iterator over all peer entries that do not carry a signature in
	/// the catalog excluding the local peer entry.
	pub fn unsigned_peers(&self) -> impl DoubleEndedIterator<Item = &PeerEntry> {
		self
			.unsigned
			.values()
			.filter(|p: &&PeerEntry| *p.id() != self.local_id)
	}

	/// Returns an iterator over all peer entries in the catalog.
	///
	/// The iterator yields both signed and unsigned entries including the local
	/// peer entry, with signed entries being the first to be returned.
	pub fn iter(&self) -> impl DoubleEndedIterator<Item = &PeerEntry> {
		self
			.signed
			.values()
			.map(|signed| signed.as_ref())
			.chain(self.unsigned.values())
	}

	/// Returns an iterator over all signed peer entries in the catalog.
	///
	/// The iterator yields entries with their signature, including the local peer
	/// entry. This is used when syncing the catalog with other peers.
	pub fn iter_signed(
		&self,
	) -> impl DoubleEndedIterator<Item = &SignedPeerEntry> {
		self.signed.values()
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

	/// Returns a reference to the signed peer entry for the given peer ID, if it
	/// exists.
	pub fn get_signed(&self, peer_id: &PeerId) -> Option<&SignedPeerEntry> {
		self.signed.get(peer_id)
	}

	/// Returns a reference to the local peer entry, if it exists.
	#[allow(clippy::missing_panics_doc)]
	pub fn local(&self) -> &SignedPeerEntry {
		self
			.get_signed(&self.local_id)
			.expect("local peer entry always exists")
	}

	/// Returns the number of peer entries in the catalog, i.e. all entries - the
	/// local peer entry.
	pub fn peers_count(&self) -> usize {
		self.signed.len() + self.unsigned.len() - 1
	}
}

/// Public Mutation API
///
/// Only unsigned entries are manually mutable through the public API, those
/// are local-only and are not synced with other peers.
impl Catalog {
	/// Inserts an unsigned peer entry in the catalog.
	///
	/// This method does not follow versioning semantics since unsigned entries
	/// are not authoritative.
	///
	/// This method does nothing if there is already a signed entry for the same
	/// peer ID.
	///
	/// Inserting the local peer entry is not allowed and always returns `false`.
	pub fn insert_unsigned(&mut self, entry: PeerEntry) -> bool {
		if entry.id() == &self.local_id {
			return false;
		}

		if !self.signed.contains_key(entry.id()) {
			self.unsigned.insert(*entry.id(), entry);
			return true;
		}

		false
	}

	/// Removes the unsigned entry for the given peer ID.
	/// Returns the removed unsigned entry if it existed.
	pub fn remove_unsigned(&mut self, peer_id: &PeerId) -> Option<PeerEntry> {
		self.unsigned.remove(peer_id)
	}

	/// Clears all unsigned entries from the catalog.
	pub fn clear_unsigned(&mut self) {
		self.unsigned.clear();
	}
}

/// The result of an attempt to insert or update a signed peer entry in the
/// catalog.
pub enum UpsertResult<'a> {
	/// A new entry was inserted.
	///
	/// This peer did not previously exist in the catalog.
	New(&'a SignedPeerEntry),

	/// An existing entry was updated with a newer version.
	/// This peer already existed in the catalog and the new
	/// entry had a higher version number.
	Updated(&'a SignedPeerEntry),

	/// The insertion was rejected because the existing entry had an equal or
	/// higher version number.
	Rejected(&'a SignedPeerEntry),
}

impl UpsertResult<'_> {
	/// Returns true if the upsert resulted in a new entry being added.
	pub fn is_new(&self) -> bool {
		matches!(self, UpsertResult::New(_))
	}

	/// Returns true if the upsert resulted in an existing entry being updated.
	pub fn is_updated(&self) -> bool {
		matches!(self, UpsertResult::Updated(_))
	}

	/// Returns true if the upsert resulted in a change to the catalog,
	/// i.e., either a new entry was added or an existing entry was updated.
	pub fn is_ok(&self) -> bool {
		self.is_new() || self.is_updated()
	}
}

/// Internal mutation API
impl Catalog {
	/// Creates a new catalog instance with the local node's peer entry as the
	/// first and only entry.
	pub(super) fn new(local: &LocalNode, config: &Config) -> Self {
		let local_entry = PeerEntry::new(local.addr().clone())
			.add_tags(config.tags.clone())
			.sign(local.secret_key())
			.expect("signing local peer entry failed.");

		let mut signed = im::OrdMap::new();
		signed.insert(local.id(), local_entry);

		Self {
			local_id: local.id(),
			signed,
			unsigned: im::OrdMap::new(),
		}
	}

	/// Inserts or updates a signed peer entry in the catalog.
	///
	/// If the catalog already contains a signed entry for this peer with a higher
	/// version number, then the insertion is rejected and a reference to the
	/// existing entry is returned as an error.
	///
	/// This method also removes any existing unsigned entry for the same peer ID.
	pub(super) fn upsert_signed(
		&mut self,
		entry: SignedPeerEntry,
	) -> UpsertResult<'_> {
		let peer_id = *entry.id();
		self.unsigned.remove(entry.id());
		match self.signed.entry(peer_id) {
			im::ordmap::Entry::Occupied(mut existing) => {
				if entry.is_newer_than(existing.get()) {
					existing.insert(entry);
					UpsertResult::Updated(
						self.signed.get(&peer_id).expect("entry exists"),
					)
				} else {
					UpsertResult::Rejected(
						self.signed.get(&peer_id).expect("entry exists"),
					)
				}
			}
			im::ordmap::Entry::Vacant(vacant) => {
				let id = *entry.id();
				vacant.insert(entry);
				UpsertResult::New(self.signed.get(&id).expect("entry exists"))
			}
		}
	}

	/// Removes all entries (signed and unsigned) for the given peer ID.
	/// Returns the removed unsigned entry if it existed.
	///
	/// Removing the local peer entry is not allowed and always returns `None`.
	pub(super) fn remove(&mut self, peer_id: &PeerId) -> Option<PeerEntry> {
		if peer_id == &self.local_id {
			return None;
		}

		if let Some(existing) = self.signed.remove(peer_id) {
			return Some(existing.into());
		}

		self.unsigned.remove(peer_id)
	}

	/// Removes the signed entry for the given peer ID.
	/// Returns the removed signed entry if it existed.
	/// Removing the local peer entry is not allowed and always returns `None`.
	pub(super) fn remove_signed(
		&mut self,
		peer_id: &PeerId,
	) -> Option<SignedPeerEntry> {
		if peer_id == &self.local_id {
			return None;
		}
		self.signed.remove(peer_id)
	}

	/// Clears all entries from the catalog except for the local peer entry.
	pub(super) fn clear(&mut self) {
		let local_entry = self
			.signed
			.get(&self.local_id)
			.expect("local peer entry always exists")
			.clone();

		self.signed.clear();
		self.signed.insert(self.local_id, local_entry);
		self.unsigned.clear();
	}

	/// Absorbs all signed entries from the given iterator into the catalog.
	///
	/// This is used when syncing catalogs between peers, it will upsert all
	/// signed entries from the other catalog into this one.
	///
	/// The local peer entry is never affected and remains unchanged.
	///
	/// Returns an iterator over events corresponding to the changes made to the
	/// catalog as a result of this operation. The events are analogous to those
	/// that would be emitted if the entries were modified individually via the
	/// announce protocol.
	pub(super) fn extend_signed(
		&mut self,
		entries: impl Iterator<Item = SignedPeerEntry>,
	) -> impl Iterator<Item = Event> {
		let mut events = Vec::new();
		for signed in entries {
			if signed.id() != &self.local_id {
				match self.upsert_signed(signed) {
					UpsertResult::New(entry) => {
						events.push(Event::PeerDiscovered(entry.clone().into_unsigned()));
					}
					UpsertResult::Updated(entry) => {
						events.push(Event::PeerUpdated(entry.clone().into_unsigned()));
					}
					UpsertResult::Rejected(_) => {}
				}
			}
		}

		events.into_iter()
	}
}
