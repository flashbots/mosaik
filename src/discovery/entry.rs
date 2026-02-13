use {
	super::Error,
	crate::{
		NetworkId,
		StreamId,
		groups::GroupId,
		network::PeerId,
		primitives::*,
		store::StoreId,
	},
	chrono::{DateTime, Utc},
	core::fmt,
	derive_more::{AsRef, Debug, Deref, Into},
	iroh::{EndpointAddr, SecretKey, Signature},
	semver::Version,
	serde::{Deserialize, Deserializer, Serialize, de},
	std::collections::BTreeSet,
};

/// Version information for a [`PeerEntry`].
///
/// The version is composed of two timestamps (in milliseconds since epoch)
/// The first timestamp represents the time when the process was
/// started and can be thought of as run-id and the second timestamp is the
/// update number withing that particular run that is updated on each change
/// to the [`PeerEntry`] and on each periodic announcement.
///
/// Peers that have not announced themselves within a certain time frame
/// (configurable via `Config::purge_after`) are considered stale and are
/// automatically removed from the catalog by the discovery system.
#[derive(
	Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub struct PeerEntryVersion(pub(crate) i64, pub(crate) i64);

impl Default for PeerEntryVersion {
	fn default() -> Self {
		Self(Utc::now().timestamp_millis(), Utc::now().timestamp_millis())
	}
}

impl core::fmt::Debug for PeerEntryVersion {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		write!(f, "{}:{}", self.0, self.1)
	}
}

impl core::fmt::Display for PeerEntryVersion {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		write!(f, "{}:{}", self.0, self.1)
	}
}

impl core::fmt::Display for Short<PeerEntryVersion> {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		write!(f, "{}", self.0.1)
	}
}

impl PeerEntryVersion {
	/// Increments the version's counter.
	#[must_use]
	pub(crate) fn increment(self) -> Self {
		let last_version = self.1.max(Utc::now().timestamp_millis());
		Self(self.0, last_version.saturating_add(1))
	}

	/// Returns the timestamp when the peer entry was last updated.
	/// Invalid timestamps default to Unix epoch.
	pub fn updated_at(&self) -> DateTime<Utc> {
		DateTime::<Utc>::from_timestamp_millis(self.1).unwrap_or_default()
	}

	/// Returns the self-declared startup time of the peer.
	///
	/// This value is not very reliable as peers may lie about their
	/// startup time, but in some contexts such as trusted groups that have
	/// trust assumptions about each other it may be used to order peers by
	/// their uptime.
	pub fn started_at(&self) -> DateTime<Utc> {
		DateTime::<Utc>::from_timestamp_millis(self.0).unwrap_or_default()
	}
}

/// Stores information about a peer in the network.
///
/// Notes:
///
/// - This is used in the discovery protocol to advertise peer information. Each
///   node is responsible for publishing its own [`PeerEntry`] to the network
///   through broadcasts whenever its state changes (e.g., new streams are
///   produced, tags are updated, local addresses discovered, etc).
///
/// - Each node signs its [`PeerEntry`] to ensure authenticity and integrity of
///   the advertised information before publishing it to the network.
///
/// - It is important to always use ordered collections in this struct to ensure
///   consistent serialization for signing and verification of equivalent
///   entries across different nodes. (e.g. `BTreeSet` for tags and streams).
///
/// - There is no public API to create a [`PeerEntry`] directly. It is intended
///   to be created by the discovery system when the network is booting and
///   received from other peers during discovery and catalog sync.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct PeerEntry {
	protocol: Version,
	network: NetworkId,
	version: PeerEntryVersion,
	address: EndpointAddr,
	tags: BTreeSet<Tag>,
	streams: BTreeSet<StreamId>,
	stores: BTreeSet<StoreId>,
	groups: BTreeSet<GroupId>,
}

/// Public query API for `PeerEntry`.
impl PeerEntry {
	/// Peer's unique identifier.
	///
	/// This identifier alone should be enough to connect to the peer. This is
	/// also the public key derived from the peer's secret key and can be used to
	/// verify signatures made by the peer.
	pub const fn id(&self) -> &PeerId {
		&self.address.id
	}

	/// The network id this peer is associated with.
	pub const fn network_id(&self) -> &NetworkId {
		&self.network
	}

	/// The `mosaik` version used by the peer.
	///
	/// This value never changes once set during peer startup.
	pub const fn protocol_version(&self) -> &Version {
		&self.protocol
	}

	/// The list of currently known transport addresses for the peer.
	pub const fn address(&self) -> &EndpointAddr {
		&self.address
	}

	/// List of opaque tags associated with the peer.
	pub const fn tags(&self) -> &BTreeSet<Tag> {
		&self.tags
	}

	/// List of streams produced by the peer.
	pub const fn streams(&self) -> &BTreeSet<StreamId> {
		&self.streams
	}

	/// List of stores available on this peer.
	pub const fn stores(&self) -> &BTreeSet<StoreId> {
		&self.stores
	}

	/// List of groups this peer is a member of.
	pub const fn groups(&self) -> &BTreeSet<GroupId> {
		&self.groups
	}

	/// The update version of the peer entry.
	///
	/// This is incremented each time the peer entry is updated.
	pub const fn update_version(&self) -> PeerEntryVersion {
		self.version
	}

	/// The timestamp when the peer entry was last updated.
	pub fn updated_at(&self) -> DateTime<Utc> {
		self.version.updated_at()
	}

	/// Computes a Blake3 digest of the `PeerEntry`.
	pub fn digest(&self) -> blake3::Hash {
		let mut hasher = blake3::Hasher::new();
		serialize_to_writer(self, &mut hasher);
		hasher.finalize()
	}

	/// Returns true if this [`PeerEntry`] is newer than the other based on the
	/// version.
	pub fn is_newer_than(&self, other: &Self) -> bool {
		self.version > other.version
	}
}

/// Public construction and mutation API for `PeerEntry`.
impl PeerEntry {
	/// Creates a new [`PeerEntry`] with the given endpoint address and empty tags
	/// and streams. There is no public API to create a [`PeerEntry`] directly. It
	/// is intended to be created by the discovery system when the network is
	/// booting.
	pub(crate) fn new(network: NetworkId, address: EndpointAddr) -> Self {
		Self {
			network,
			address,
			tags: BTreeSet::new(),
			streams: BTreeSet::new(),
			stores: BTreeSet::new(),
			groups: BTreeSet::new(),
			version: PeerEntryVersion::default(),
			protocol: env!("CARGO_PKG_VERSION")
				.parse()
				.expect("Invalid CARGO_PKG_VERSION for mosaik"),
		}
	}

	/// Updates the transport address of the peer.
	pub fn update_address(
		mut self,
		address: EndpointAddr,
	) -> Result<Self, Error> {
		if address.id != *self.id() {
			return Err(Error::PeerIdChanged(*self.id(), address.id));
		}

		self.address = address;
		self.version = self.version.increment();
		Ok(self)
	}

	/// Adds stream id(s) to the list of streams produced by the peer.
	#[must_use]
	pub fn add_streams<V>(
		mut self,
		streams: impl IntoIterOrSingle<StreamId, V>,
	) -> Self {
		let count = self.streams.len();
		self.streams.extend(streams.iterator());

		if count != self.streams.len() {
			self.version = self.version.increment();
		}

		self
	}

	/// Removes stream id(s) from the list of streams produced by the peer.
	#[must_use]
	pub fn remove_streams<V>(
		mut self,
		streams: impl IntoIterOrSingle<StreamId, V>,
	) -> Self {
		let mut was_present = false;
		for stream in streams.iterator() {
			was_present |= self.streams.remove(&stream);
		}

		if was_present {
			self.version = self.version.increment();
		}

		self
	}

	/// Adds store id(s) to the list of stores available on the peer.
	#[must_use]
	pub fn add_stores<V>(
		mut self,
		stores: impl IntoIterOrSingle<StoreId, V>,
	) -> Self {
		let count = self.stores.len();
		self.stores.extend(stores.iterator());

		if count != self.stores.len() {
			self.version = self.version.increment();
		}

		self
	}

	/// Removes store id(s) from the list of stores available on the peer.
	#[must_use]
	pub fn remove_stores<V>(
		mut self,
		stores: impl IntoIterOrSingle<StoreId, V>,
	) -> Self {
		let mut was_present = false;
		for store in stores.iterator() {
			was_present |= self.stores.remove(&store);
		}

		if was_present {
			self.version = self.version.increment();
		}

		self
	}

	/// Adds group id(s) to the list of groups this peer is a member of.
	#[must_use]
	pub fn add_groups<V>(
		mut self,
		groups: impl IntoIterOrSingle<GroupId, V>,
	) -> Self {
		let count = self.groups.len();
		self.groups.extend(groups.iterator());

		if count != self.groups.len() {
			self.version = self.version.increment();
		}

		self
	}

	/// Removes group id(s) from the list of groups this peer is a member of.
	#[must_use]
	pub fn remove_groups<V>(
		mut self,
		groups: impl IntoIterOrSingle<GroupId, V>,
	) -> Self {
		let mut was_present = false;
		for group in groups.iterator() {
			was_present |= self.groups.remove(&group);
		}

		if was_present {
			self.version = self.version.increment();
		}

		self
	}

	/// Adds tag(s) to the list of tags associated with the peer.
	#[must_use]
	pub fn add_tags<V>(mut self, tags: impl IntoIterOrSingle<Tag, V>) -> Self {
		let count = self.tags.len();
		self.tags.extend(tags.iterator());

		if count != self.tags.len() {
			self.version = self.version.increment();
		}

		self
	}

	/// Removes tag(s) from the list of tags associated with the peer.
	#[must_use]
	pub fn remove_tags<V>(mut self, tags: impl IntoIterOrSingle<Tag, V>) -> Self {
		let mut was_present = false;
		for tag in tags.iterator() {
			was_present |= self.tags.remove(&tag);
		}

		if was_present {
			self.version = self.version.increment();
		}

		self
	}

	/// Increments the version of the peer entry without making any other changes.
	#[must_use]
	pub(crate) fn increment_version(mut self) -> Self {
		self.version = self.version.increment();
		self
	}

	/// Signs the [`PeerEntry`] with the given secret key, producing a
	/// [`SignedPeerEntry`] that can be verified by other peers.
	///
	/// The signature is over the digest of the [`PeerEntry`] as computed by the
	/// [`PeerEntry::digest`] method.
	pub fn sign(self, secret: &SecretKey) -> Result<SignedPeerEntry, Error> {
		let actual_id: PeerId = *self.id();
		let expected_id: PeerId = secret.public();

		if actual_id != expected_id {
			return Err(Error::InvalidSecretKey(expected_id, actual_id));
		}

		let digest = self.digest();
		let signature = secret.sign(digest.as_bytes());

		Ok(SignedPeerEntry(self, signature))
	}
}

impl From<&PeerEntry> for PeerId {
	fn from(entry: &PeerEntry) -> Self {
		*entry.id()
	}
}

impl fmt::Display for Short<&PeerEntry> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			f,
			"PeerEntry[#{}]({}, tags: {}, streams: {})",
			Short(self.0.update_version()),
			Short(self.0.id()),
			FmtIter::<Short<_>, _>::new(&self.0.tags),
			FmtIter::<Short<_>, _>::new(&self.0.streams),
		)
	}
}

impl fmt::Debug for Pretty<'_, PeerEntry> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		writeln!(f, "PeerEntry:")?;
		writeln!(f, "  id: {}", self.id())?;
		writeln!(f, "  network: {}", self.network_id())?;
		writeln!(
			f,
			"  ips: {:#?}",
			&self.address.ip_addrs().collect::<Vec<_>>()
		)?;

		writeln!(
			f,
			"  relays: {:?}",
			&self
				.address
				.relay_urls()
				.map(|r| r.as_str())
				.collect::<Vec<_>>()
		)?;

		writeln!(f, "  tags: {}", FmtIter::<Short<_>, _>::new(&self.tags))?;
		writeln!(f, "  groups: {}", FmtIter::<Short<_>, _>::new(&self.groups))?;
		writeln!(
			f,
			"  streams: {}",
			FmtIter::<Short<_>, _>::new(&self.streams)
		)?;

		writeln!(f, "  stores: {}", FmtIter::<Short<_>, _>::new(&self.stores))?;
		writeln!(f, "  update: {}", self.update_version())?;
		writeln!(f, "  protocol: {}", self.protocol)
	}
}

/// A [`PeerEntry`] along with its signature signed by the peer's secret key.
///
/// Notes:
///
/// - This is the structure that is broadcasted in the discovery protocol to
///   advertise a peer's information or during catalog sync.
///
/// - When a [`SignedPeerEntry`] is received, peers should verify the signature
///   against the peer id before accepting and processing the entry.
///
/// - The discovery protocol should never accept unsigned entries. Unsigned
///   entries may be only used locally by the node for testing or other internal
///   purposes but they should never be published to the network.
///
/// - There is no way to create an invalid `SignedPeerEntry` as the
///   deserialization always verifies the signature and fails if invalid and the
///   api only allows creating signed entries through the `sign` method of
///   `PeerEntry`.
#[derive(Debug, Clone, Serialize, Deref, AsRef, Into, PartialEq, Eq)]
pub struct SignedPeerEntry(
	#[deref] PeerEntry,
	#[debug("signature: {}", Abbreviated::<16, _>(_1.to_bytes()))] Signature,
);

impl SignedPeerEntry {
	/// Consumes the `SignedPeerEntry`, returning the inner `PeerEntry`.
	pub fn into_unsigned(self) -> PeerEntry {
		self.0
	}
}

impl SignedPeerEntry {
	/// Verifies the signature of the `SignedPeerEntry`, returning true if
	/// valid.
	fn is_signature_valid(&self) -> bool {
		let digest = self.0.digest();
		self.0.id().verify(digest.as_bytes(), &self.1).is_ok()
	}

	/// Verifies the signature of the `SignedPeerEntry`, returning an error if
	/// invalid.
	fn verify_signature(&self) -> Result<(), Error> {
		self
			.is_signature_valid()
			.then_some(())
			.ok_or(Error::InvalidSignature)
	}
}

impl From<&SignedPeerEntry> for PeerId {
	fn from(entry: &SignedPeerEntry) -> Self {
		*entry.id()
	}
}

/// Ensure that deserialization of `SignedPeerEntry` always verifies the
/// signature and it is not possible to create an invalid instance of this type.
impl<'de> Deserialize<'de> for SignedPeerEntry {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: Deserializer<'de>,
	{
		let (entry, signature) =
			<(PeerEntry, Signature)>::deserialize(deserializer)?;
		let signed = Self(entry, signature);
		signed.verify_signature().map_err(de::Error::custom)?;
		Ok(signed)
	}
}

impl From<SignedPeerEntry> for PeerEntry {
	fn from(signed: SignedPeerEntry) -> Self {
		signed.0
	}
}

impl From<&SignedPeerEntry> for PeerEntry {
	fn from(signed: &SignedPeerEntry) -> Self {
		signed.clone().0
	}
}

impl fmt::Debug for Pretty<'_, SignedPeerEntry> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "Signed{:?}", Pretty(&self.0.0))?;
		writeln!(
			f,
			"  signature: {}",
			Abbreviated::<16, _>(self.1.to_bytes())
		)
	}
}

impl fmt::Display for Short<&SignedPeerEntry> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			f,
			"SignedPeerEntry[#{}]({}, tags: {}, streams: {}, groups: {})",
			Short(self.0.update_version()),
			Short(self.0.id()),
			FmtIter::<Short<_>, _>::new(&self.0.tags),
			FmtIter::<Short<_>, _>::new(&self.0.streams),
			FmtIter::<Short<_>, _>::new(&self.0.groups),
		)
	}
}

impl fmt::Display for Pretty<'_, EndpointAddr> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{:?}", self.addrs)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn signed_peer_entry_with_invalid_signature_fails_to_deserialize() {
		let network_id = NetworkId::random();
		let secret = SecretKey::generate(&mut rand::rng());
		let address = EndpointAddr::from(secret.public());
		let entry = PeerEntry::new(network_id, address);
		let signed = entry.sign(&secret).unwrap();

		// Serialize the valid signed entry
		let serialized = serialize(&signed).to_vec();

		// Tamper with the signature bytes (last 64 bytes are the signature)
		let mut tampered = serialized;
		let len = tampered.len();
		tampered[len - 1] ^= 0xFF; // Flip bits in signature

		// Attempt to deserialize should fail
		let result: Result<SignedPeerEntry, _> = deserialize(&tampered);

		assert!(
			result.is_err(),
			"Expected deserialization to fail with invalid signature"
		);
	}

	#[test]
	fn signed_peer_entry_with_modified_entry_fails_to_deserialize() {
		let network_id = NetworkId::random();
		let secret = SecretKey::generate(&mut rand::rng());
		let address = EndpointAddr::from(secret.public());
		let entry = PeerEntry::new(network_id, address.clone())
			.add_tags(Tag::from("original"));
		let signed = entry.sign(&secret).unwrap();

		// Get the signature from valid signed entry
		let signature = signed.1;

		// Create modified entry with different tags
		let modified_entry =
			PeerEntry::new(network_id, address).add_tags(Tag::from("modified"));

		// Manually construct modified entry but original signature
		let invalid_signed = SignedPeerEntry(modified_entry, signature);
		let serialized = serialize(&invalid_signed);

		// Attempt to deserialize should fail
		let result: Result<SignedPeerEntry, _> = deserialize(&serialized);

		assert!(
			result.is_err(),
			"Expected deserialization to fail with modified entry"
		);
	}

	#[test]
	fn valid_signed_peer_entry_deserializes_successfully() {
		let network_id = NetworkId::random();
		let secret = SecretKey::generate(&mut rand::rng());
		let address = EndpointAddr::from(secret.public());
		let entry = PeerEntry::new(network_id, address);
		let signed = entry.sign(&secret).unwrap();

		let serialized = serialize(&signed);

		let deserialized: SignedPeerEntry = deserialize(&serialized)
			.expect("Failed to deserialize valid SignedPeerEntry");

		assert_eq!(signed, deserialized);
	}

	#[test]
	fn version_increments_on_add_tags() {
		let network_id = NetworkId::random();
		let secret = SecretKey::generate(&mut rand::rng());
		let address = EndpointAddr::from(secret.public());
		let entry = PeerEntry::new(network_id, address);
		let initial_version = entry.update_version();

		let updated = entry.add_tags(Tag::from("test"));

		assert!(
			updated.update_version() > initial_version,
			"Version should increment after add_tags"
		);
	}

	#[test]
	fn version_increments_on_remove_tags() {
		let network_id = NetworkId::random();
		let secret = SecretKey::generate(&mut rand::rng());
		let address = EndpointAddr::from(secret.public());
		let entry = PeerEntry::new(network_id, address).add_tags(Tag::from("test"));
		let initial_version = entry.update_version();

		let updated = entry.remove_tags("test");

		assert!(
			updated.update_version() > initial_version,
			"Version should increment after remove_tags"
		);
	}

	#[test]
	fn version_increments_on_add_streams() {
		let network_id = NetworkId::random();
		let secret = SecretKey::generate(&mut rand::rng());
		let address = EndpointAddr::from(secret.public());
		let entry = PeerEntry::new(network_id, address);
		let initial_version = entry.update_version();

		let updated = entry.add_streams(StreamId::from("test-stream"));

		assert!(
			updated.update_version() > initial_version,
			"Version should increment after add_streams"
		);
	}

	#[test]
	fn version_increments_on_remove_streams() {
		let network_id = NetworkId::random();
		let secret = SecretKey::generate(&mut rand::rng());
		let address = EndpointAddr::from(secret.public());
		let entry = PeerEntry::new(network_id, address)
			.add_streams(StreamId::from("test-stream"));
		let initial_version = entry.update_version();

		let updated = entry.remove_streams(StreamId::from("test-stream"));

		assert!(
			updated.update_version() > initial_version,
			"Version should increment after remove_streams"
		);
	}

	#[test]
	fn version_increments_on_update_address() {
		let network_id = NetworkId::random();
		let secret = SecretKey::generate(&mut rand::rng());
		let address = EndpointAddr::from(secret.public());
		let entry = PeerEntry::new(network_id, address.clone());
		let initial_version = entry.update_version();

		let updated = entry.update_address(address).unwrap();

		assert!(
			updated.update_version() > initial_version,
			"Version should increment after update_address"
		);
	}

	#[test]
	fn version_increments_monotonically_on_multiple_changes() {
		let network_id = NetworkId::random();
		let secret = SecretKey::generate(&mut rand::rng());
		let address = EndpointAddr::from(secret.public());
		let entry = PeerEntry::new(network_id, address.clone());
		let v0 = entry.update_version();

		let entry = entry.add_tags(Tag::from("tag1"));
		let v1 = entry.update_version();
		assert!(v1 > v0, "Version should increment after first change");

		let entry = entry.add_streams(StreamId::from("stream1"));
		let v2 = entry.update_version();
		assert!(v2 > v1, "Version should increment after second change");

		let entry = entry.remove_tags(Tag::from("tag1"));
		let v3 = entry.update_version();
		assert!(v3 > v2, "Version should increment after third change");

		let entry = entry.update_address(address).unwrap();
		let v4 = entry.update_version();
		assert!(v4 > v3, "Version should increment after fourth change");
	}

	#[test]
	fn is_newer_than_returns_correct_result() {
		let network_id = NetworkId::random();
		let secret = SecretKey::generate(&mut rand::rng());
		let address = EndpointAddr::from(secret.public());
		let entry1 = PeerEntry::new(network_id, address);
		let entry2 = entry1.clone().add_tags(Tag::from("test"));

		assert!(
			entry2.is_newer_than(&entry1),
			"Updated entry should be newer than original"
		);
		assert!(
			!entry1.is_newer_than(&entry2),
			"Original entry should not be newer than updated"
		);
		assert!(
			!entry1.is_newer_than(&entry1),
			"Entry should not be newer than itself"
		);
	}
}
