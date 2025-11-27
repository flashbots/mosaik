use {
	crate::datum::StreamId,
	bytes::Bytes,
	core::ops::Deref,
	derive_more::{Deref, From, Into},
	iroh::{EndpointAddr, Signature},
	serde::{Deserialize, Serialize},
	sha3::{Digest as _, Sha3_256},
	std::collections::BTreeSet,
};

/// This type uniquely identifies a peer in the network.
pub type PeerId = iroh::EndpointId;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PeerInfo {
	address: EndpointAddr,
	tags: BTreeSet<Tag>,
	streams: BTreeSet<StreamId>,
}

impl PeerInfo {
	pub fn new(address: EndpointAddr) -> Self {
		Self {
			address,
			tags: BTreeSet::new(),
			streams: BTreeSet::new(),
		}
	}

	pub fn new_with_streams(
		address: EndpointAddr,
		streams: BTreeSet<StreamId>,
	) -> Self {
		Self {
			address,
			tags: BTreeSet::new(),
			streams,
		}
	}

	pub const fn id(&self) -> &PeerId {
		&self.address.id
	}

	pub fn address(&self) -> &EndpointAddr {
		&self.address
	}

	pub fn streams(&self) -> &BTreeSet<StreamId> {
		&self.streams
	}

	pub fn tags(&self) -> &BTreeSet<Tag> {
		&self.tags
	}

	/// Computes a digest of the `PeerInfo`.
	///
	/// # Panics
	///
	/// This function will panic if serialization fails, which should not happen.
	pub fn digest(&self) -> Digest {
		let serialized = rmp_serde::to_vec(self).expect("infallible");
		Digest(Sha3_256::digest(&serialized).into())
	}

	pub fn sign(self, signer: &iroh::SecretKey) -> SignedPeerInfo {
		let digest = self.digest();
		SignedPeerInfo {
			info: self,
			signature: signer.sign(&*digest),
		}
	}

	#[must_use]
	pub fn add_stream(mut self, stream_id: StreamId) -> Self {
		self.streams.insert(stream_id);
		self
	}

	#[must_use]
	pub fn add_tag(mut self, tag: impl Into<Tag>) -> Self {
		self.tags.insert(tag.into());
		self
	}

	#[must_use]
	pub fn add_tags(
		mut self,
		tags: impl Iterator<Item = impl Into<Tag>>,
	) -> Self {
		self.tags.extend(tags.map(Into::into));
		self
	}

	#[must_use]
	pub fn remove_tag(mut self, tag: &Tag) -> Self {
		self.tags.remove(tag);
		self
	}

	#[must_use]
	pub fn update_address(mut self, address: EndpointAddr) -> Self {
		self.address = address;
		self
	}

	pub fn into_address(self) -> EndpointAddr {
		self.address
	}

	#[cfg(test)]
	pub fn random() -> Self {
		use iroh::SecretKey;

		let secret = SecretKey::generate(&mut rand::rng());
		let public = secret.public();
		let address = EndpointAddr::new(public);
		let streams = BTreeSet::new();
		let tags = BTreeSet::new();

		Self {
			address,
			tags,
			streams,
		}
	}
}

impl core::fmt::Display for PeerInfo {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		write!(f, "{}", self.id())
	}
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SignedPeerInfo {
	pub info: PeerInfo,
	pub signature: Signature,
}

impl SignedPeerInfo {
	pub fn verify(&self) -> bool {
		let pubkey = self.info.address.id;
		let digest = self.info.digest();
		pubkey.verify(&*digest, &self.signature).is_ok()
	}

	pub fn into_peer_info(self) -> PeerInfo {
		self.info
	}

	pub(crate) fn from_bytes(
		bytes: &[u8],
	) -> Result<Self, rmp_serde::decode::Error> {
		rmp_serde::from_slice(bytes)
	}

	pub(crate) fn into_bytes(self) -> Result<Bytes, rmp_serde::encode::Error> {
		rmp_serde::to_vec(&self).map(Bytes::from)
	}
}

impl Deref for SignedPeerInfo {
	type Target = PeerInfo;

	fn deref(&self) -> &Self::Target {
		&self.info
	}
}

impl From<SignedPeerInfo> for PeerInfo {
	fn from(signed: SignedPeerInfo) -> Self {
		signed.info
	}
}

impl core::fmt::Debug for SignedPeerInfo {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		f.debug_struct("SignedPeerInfo")
			.field("info", &self.info)
			.field(
				"signature",
				&self.signature.to_string().to_ascii_lowercase(),
			)
			.finish()
	}
}

impl core::fmt::Display for SignedPeerInfo {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		let status = if self.verify() { " [verified]" } else { "" };
		write!(f, "{}{}", self.info.id(), status)
	}
}

#[derive(
	Debug,
	Clone,
	PartialEq,
	Eq,
	PartialOrd,
	Ord,
	Hash,
	Serialize,
	Deserialize,
	From,
	Into,
	Deref,
)]
pub struct Digest(pub [u8; 32]);

/// A tag is an opaque 32-byte hash that can be used to label peers.
#[derive(
	PartialEq,
	Eq,
	PartialOrd,
	Ord,
	Hash,
	Clone,
	Serialize,
	Deserialize,
	From,
	Into,
	Deref,
)]
pub struct Tag(Digest);

impl core::fmt::Debug for Tag {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		for byte in &self.0.0[..4] {
			write!(f, "{byte:02x}")?;
		}
		Ok(())
	}
}

impl core::fmt::Display for Tag {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		for byte in &self.0.0[..4] {
			write!(f, "{byte:02x}")?;
		}
		Ok(())
	}
}

impl Tag {
	/// Creates a new tag from arbitrary data by hashing it.
	pub fn new<D: AsRef<[u8]>>(data: D) -> Self {
		let mut hasher = Sha3_256::new();
		hasher.update(data.as_ref());
		let result = hasher.finalize();
		Tag(Digest(result.into()))
	}
}

impl<T: AsRef<str>> From<T> for Tag {
	fn from(s: T) -> Self {
		Tag::new(s.as_ref().as_bytes())
	}
}
