use {
	core::{convert::Infallible, fmt, str::FromStr},
	derive_more::Deref,
	serde::{Deserialize, Deserializer, Serialize, de},
	sha3::{Digest, Sha3_256},
};

/// This type is used to uniquely identify various entities in the Mosaik
/// ecosystem such as streams, peers, and networks. It is represented as a
/// 32-byte array, typically derived from a SHA3-256 hash of some preimage.
///
/// All unique ids in Mosaik can be derived from strings and byte slices using
/// the `From` trait implementations provided.
///
/// Notes:
///  - when serialized to human readable formats (e.g., JSON), `UniqueId`s are
///    represented as hex-encoded strings.
///  - when serialized to binary formats (e.g., bincode), `UniqueId`s are
///    represented as raw 32-byte arrays.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deref)]
pub struct UniqueId([u8; 32]);

/// A tag is an opaque 32-byte hash that can be used to label peers, networks,
/// resources and other things. Its use is context dependent.
pub type Tag = UniqueId;

impl<T: AsRef<str>> From<T> for UniqueId {
	fn from(s: T) -> Self {
		let hash = Sha3_256::digest(s.as_ref());
		UniqueId(hash.into())
	}
}

impl From<UniqueId> for [u8; 32] {
	fn from(id: UniqueId) -> Self {
		id.0
	}
}

impl From<&UniqueId> for [u8; 32] {
	fn from(id: &UniqueId) -> Self {
		id.0
	}
}

impl FromStr for UniqueId {
	type Err = Infallible;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		Ok(UniqueId::from(s))
	}
}

impl fmt::Debug for UniqueId {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}", hex::encode(self.0))
	}
}

impl fmt::Display for UniqueId {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}", hex::encode(self.0))
	}
}

impl Serialize for UniqueId {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
	where
		S: serde::Serializer,
	{
		if serializer.is_human_readable() {
			serializer.serialize_str(hex::encode(self.0).as_str())
		} else {
			self.0.serialize(serializer)
		}
	}
}

impl<'de> Deserialize<'de> for UniqueId {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: Deserializer<'de>,
	{
		if deserializer.is_human_readable() {
			let s = String::deserialize(deserializer)?;
			let bytes = hex::decode(s).map_err(de::Error::custom)?;
			if bytes.len() != 32 {
				return Err(de::Error::custom("invalid length for UniqueId"));
			}
			let mut array = [0u8; 32];
			array.copy_from_slice(&bytes);
			Ok(UniqueId(array))
		} else {
			let bytes = <[u8; 32]>::deserialize(deserializer)?;
			Ok(UniqueId(bytes))
		}
	}
}

impl UniqueId {
	/// Returns the byte representation of the unique id.
	pub fn as_bytes(&self) -> &[u8; 32] {
		&self.0
	}

	/// Generates a random unique id.
	pub fn random() -> Self {
		UniqueId(rand::random())
	}
}
