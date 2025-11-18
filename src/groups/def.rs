use {
	crate::prelude::{NetworkId, PeerId},
	serde::{Deserialize, Serialize},
	sha3::{Digest, Sha3_256},
	std::collections::BTreeSet,
};

#[derive(Debug, Serialize, Deserialize)]
pub struct GroupDef {
	pub network_id: NetworkId,
	pub seed: [u8; 32],
	pub redundancy: u32,
}

impl GroupDef {
	pub fn digest(&self) -> GroupHash {
		let mut hasher = Sha3_256::default();
		hasher.update(&self.network_id.as_bytes());
		hasher.update(&self.seed);
		hasher.update(&self.redundancy.to_be_bytes());
		GroupHash(hasher.finalize().into())
	}
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GroupHash([u8; 32]);

pub struct GroupState {
	leaders: BTreeSet<PeerId>,
	members: BTreeSet<PeerId>,
}
