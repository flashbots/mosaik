use {
	crate::datum::StreamId,
	chrono::Utc,
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
	pub address: EndpointAddr,
	pub streams: BTreeSet<StreamId>,
}

impl PeerInfo {
	pub const fn id(&self) -> &PeerId {
		&self.address.id
	}

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

	pub fn add_stream(mut self, stream_id: StreamId) -> Self {
		self.streams.insert(stream_id);
		self
	}

	pub fn update_address(mut self, address: EndpointAddr) -> Self {
		self.address = address;
		self
	}

	#[cfg(test)]
	pub fn random() -> Self {
		use iroh::SecretKey;

		let secret = SecretKey::generate(&mut rand::rng());
		let public = secret.public();
		let address = EndpointAddr::new(public);
		let streams = BTreeSet::new();

		Self { address, streams }
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateId {
	pub run_id: u64,
	pub seq: u64,
}

impl Default for UpdateId {
	fn default() -> Self {
		Self {
			run_id: Utc::now().timestamp_millis() as u64,
			seq: 1,
		}
	}
}

impl UpdateId {
	pub fn next(&self) -> Self {
		Self {
			run_id: self.run_id,
			seq: self.seq.wrapping_add(1),
		}
	}
}

impl Ord for UpdateId {
	fn cmp(&self, other: &Self) -> core::cmp::Ordering {
		self
			.run_id
			.cmp(&other.run_id)
			.then_with(|| self.seq.cmp(&other.seq))
	}
}

impl PartialOrd for UpdateId {
	fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
		Some(self.cmp(other))
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
