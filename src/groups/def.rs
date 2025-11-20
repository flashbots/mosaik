use {
	crate::prelude::{NetworkId, PeerId},
	core::{fmt, str::FromStr},
	derive_more::{From, Into},
	serde::{Deserialize, Serialize},
	sha3::{Digest, Sha3_256},
	std::collections::BTreeSet,
};

#[derive(Clone, Copy, Serialize, Deserialize, From, Into)]
pub struct GroupKey([u8; 32]);

impl AsRef<[u8]> for GroupKey {
	fn as_ref(&self) -> &[u8] {
		&self.0
	}
}

impl FromStr for GroupKey {
	type Err = hex::FromHexError;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		let bytes = match hex::decode(s) {
			Ok(bytes) if bytes.len() == 32 => {
				return Ok(GroupKey(
					bytes.try_into().expect("slice with incorrect length"),
				));
			}
			_ => Sha3_256::digest(s.as_bytes()).into(),
		};

		Ok(GroupKey(bytes))
	}
}

impl fmt::Display for GroupKey {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			f,
			"{}..{}",
			hex::encode(&self.0[0..2]),
			hex::encode(&self.0[30..32])
		)
	}
}

impl fmt::Debug for GroupKey {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "GroupKey({self})")
	}
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GroupDef {
	pub network_id: NetworkId,
	pub key: GroupKey,
}

impl GroupDef {
	pub fn digest(&self) -> GroupHash {
		let mut hasher = Sha3_256::default();
		hasher.update(&self.network_id);
		hasher.update(self.key);
		GroupHash(hasher.finalize().into())
	}
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GroupHash([u8; 32]);

pub struct GroupState {
	pub leaders: BTreeSet<PeerId>,
	pub members: BTreeSet<PeerId>,
}
