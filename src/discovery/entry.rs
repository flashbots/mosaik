use {
	super::Error,
	crate::{EndpointAddr, PeerId, SecretKey, Signature, StreamId, Tag},
	bincode::{config::standard, serde::encode_into_std_write},
	derive_more::{AsRef, Debug, Deref, Into},
	serde::{Deserialize, Deserializer, Serialize, de},
	std::collections::BTreeSet,
};

/// Version information for a `PeerEntry`.
///
/// The version is composed of a timestamp (in milliseconds since epoch)
/// and a counter. The timestamp represents the time when the process was
/// started and can be thought of as run-id and the counter is the update number
/// withing that particular run.
#[derive(
	Debug,
	Clone,
	Copy,
	Serialize,
	Deserialize,
	PartialEq,
	Eq,
	PartialOrd,
	Ord,
	Hash,
)]
pub struct PeerEntryVersion(i64, u64);

impl Default for PeerEntryVersion {
	fn default() -> Self {
		Self(chrono::Utc::now().timestamp_millis(), 0)
	}
}

impl PeerEntryVersion {
	/// Increments the version's counter.
	#[must_use]
	pub fn increment(self) -> Self {
		Self(self.0, self.1.saturating_add(1))
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
	version: PeerEntryVersion,
	address: EndpointAddr,
	tags: BTreeSet<Tag>,
	streams: BTreeSet<StreamId>,
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

	/// The version of the peer entry.
	///
	/// This is incremented each time the peer entry is updated.
	pub const fn version(&self) -> PeerEntryVersion {
		self.version
	}

	/// Computes a SHA3-356 digest of the `PeerEntry`.
	pub fn digest(&self) -> [u8; 32] {
		use sha3::{Digest as _, Sha3_256};
		let mut hasher = Sha3_256::new();
		#[expect(clippy::missing_panics_doc)]
		encode_into_std_write(self, &mut hasher, standard())
			.expect("Failed to encode PeerEntry for digest");
		hasher.finalize().into()
	}
}

/// Public construction and mutation API for `PeerEntry`.
impl PeerEntry {
	/// Creates a new [`PeerEntry`] with the given endpoint address and empty tags
	/// and streams. There is no public API to create a [`PeerEntry`] directly. It
	/// is intended to be created by the discovery system when the network is
	/// booting.
	pub(crate) fn new(address: EndpointAddr) -> Self {
		Self {
			address,
			tags: BTreeSet::new(),
			streams: BTreeSet::new(),
			version: PeerEntryVersion::default(),
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

	/// Adds a stream id to the list of streams produced by the peer.
	#[must_use]
	pub fn add_stream(mut self, stream: impl Into<StreamId>) -> Self {
		self.streams.insert(stream.into());
		self.version = self.version.increment();
		self
	}

	/// Removes a stream id from the list of streams produced by the peer.
	#[must_use]
	pub fn remove_stream(mut self, stream: impl Into<StreamId>) -> Self {
		self.streams.remove(&stream.into());
		self.version = self.version.increment();
		self
	}

	/// Adds a tag to the list of tags associated with the peer.
	#[must_use]
	pub fn add_tag(mut self, tag: impl Into<Tag>) -> Self {
		self.tags.insert(tag.into());
		self.version = self.version.increment();
		self
	}

	/// Adds a set of tags to the list of tags associated with the peer.
	#[must_use]
	pub fn add_tags(
		mut self,
		tags: impl IntoIterator<Item = impl Into<Tag>>,
	) -> Self {
		self.tags.extend(tags.into_iter().map(Into::into));
		self.version = self.version.increment();
		self
	}

	/// Removes a tag from the list of tags associated with the peer.
	#[must_use]
	pub fn remove_tag(mut self, tag: impl Into<Tag>) -> Self {
		self.tags.remove(&tag.into());
		self.version = self.version.increment();
		self
	}

	/// Removes a set of tags from the list of tags associated with the peer.
	#[must_use]
	pub fn remove_tags(
		mut self,
		tags: impl IntoIterator<Item = impl Into<Tag>>,
	) -> Self {
		for tag in tags {
			self.tags.remove(&tag.into());
		}
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

		let signature = secret.sign(&self.digest());

		Ok(SignedPeerEntry(self, signature))
	}
}

/// A [`PeerEntry`] along with its signature signed by the peer's secret key.
///
/// Notes:
///
/// - This is the structure that is actually broadcasted in the discovery
///   protocol to advertise a peer's information or during catalog sync.
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
#[derive(Debug, Clone, Serialize, Deref, AsRef, Into, PartialEq)]
pub struct SignedPeerEntry(
	#[deref] PeerEntry,
	#[debug("signature: {}", hex::encode(_1.to_bytes()))] Signature,
);

impl SignedPeerEntry {
	/// Consumes the `SignedPeerEntry`, returning the inner `PeerEntry`.
	pub fn unsigned(self) -> PeerEntry {
		self.0
	}
}

impl SignedPeerEntry {
	/// Verifies the signature of the `SignedPeerEntry`, returning true if
	/// valid.
	fn is_signature_valid(&self) -> bool {
		let digest = self.0.digest();
		self.0.id().verify(&digest, &self.1).is_ok()
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

/// Ensure that deserialization of `SignedPeerEntry` always verifies the
/// signature and it is not possible to create an invalid instance of this type.
impl<'de> Deserialize<'de> for SignedPeerEntry {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: Deserializer<'de>,
	{
		let (entry, signature) =
			<(PeerEntry, Signature)>::deserialize(deserializer)?;
		let signed = SignedPeerEntry(entry, signature);
		signed.verify_signature().map_err(de::Error::custom)?;
		Ok(signed)
	}
}

impl From<SignedPeerEntry> for PeerEntry {
	fn from(signed: SignedPeerEntry) -> Self {
		signed.0
	}
}
