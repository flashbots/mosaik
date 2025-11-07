use {
	chrono::Utc,
	derive_more::{Deref, From, Into},
	iroh::{EndpointAddr, Signature},
	serde::{Deserialize, Serialize},
	sha3::{Digest as _, Sha3_256},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
	pub address: EndpointAddr,
	pub version: UpdateId,
}

impl PeerInfo {
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
