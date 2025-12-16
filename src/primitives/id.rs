use {
	crate::primitives::Short,
	core::{convert::Infallible, fmt, str::FromStr},
	derive_more::Deref,
	serde::{Deserialize, Deserializer, Serialize, de},
};

/// This type is used to uniquely identify various entities in the Mosaik
/// ecosystem such as streams, peers, and networks. It is represented as a
/// 32-byte array, typically derived from a Blake3 hash of some preimage.
///
/// All unique ids in Mosaik can be derived from strings and byte slices using
/// the `From` trait implementations provided.
///
/// Notes:
///  - when serialized to human readable formats (e.g., JSON), `UniqueId`s are
///    represented as hex-encoded strings.
///  - when serialized to binary formats (e.g., bincode), `UniqueId`s are
///    represented as raw 32-byte arrays.
#[derive(Clone, Copy, Deref)]
pub struct UniqueId(blake3::Hash);

/// A tag is an opaque 32-byte hash that can be used to label peers, networks,
/// resources and other things. Its use is context dependent.
pub type Tag = UniqueId;

impl<T: AsRef<str>> From<T> for UniqueId {
	fn from(s: T) -> Self {
		let s = s.as_ref();
		// if the string is already a 32-byte hex string, decode it directly
		// otherwise, hash it to produce the unique id
		match hex::decode(s) {
			Ok(b) if b.len() == 32 => {
				UniqueId(blake3::Hash::from_slice(&b).expect("slice is 32 bytes"))
			}
			_ => UniqueId(blake3::hash(s.as_bytes())),
		}
	}
}

impl PartialEq for UniqueId {
	fn eq(&self, other: &Self) -> bool {
		self.0 == other.0
	}
}
impl Eq for UniqueId {}

impl PartialOrd for UniqueId {
	fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
		Some(self.cmp(other))
	}
}
impl Ord for UniqueId {
	fn cmp(&self, other: &Self) -> core::cmp::Ordering {
		self.0.as_bytes().cmp(other.0.as_bytes())
	}
}

impl core::hash::Hash for UniqueId {
	fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
		self.0.as_bytes().hash(state);
	}
}

impl AsRef<[u8]> for UniqueId {
	fn as_ref(&self) -> &[u8] {
		self.0.as_bytes()
	}
}

impl From<UniqueId> for [u8; 32] {
	fn from(id: UniqueId) -> Self {
		*id.0.as_bytes()
	}
}

impl From<&UniqueId> for [u8; 32] {
	fn from(id: &UniqueId) -> Self {
		*id.0.as_bytes()
	}
}

impl PartialEq<&UniqueId> for UniqueId {
	fn eq(&self, other: &&UniqueId) -> bool {
		self.0 == other.0
	}
}

impl PartialEq<UniqueId> for &UniqueId {
	fn eq(&self, other: &UniqueId) -> bool {
		self.0 == other.0
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
		write!(f, "{}", self.0.to_hex())
	}
}

impl fmt::Display for UniqueId {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}", Short(self.0.as_bytes()))
	}
}

impl Serialize for UniqueId {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
	where
		S: serde::Serializer,
	{
		if serializer.is_human_readable() {
			serializer.serialize_str(self.0.to_hex().as_str())
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
			Ok(UniqueId(
				blake3::Hash::from_hex(&s).map_err(de::Error::custom)?,
			))
		} else {
			let bytes = <[u8; 32]>::deserialize(deserializer)?;
			Ok(UniqueId(blake3::Hash::from_bytes(bytes)))
		}
	}
}

impl UniqueId {
	/// Returns the byte representation of the unique id.
	pub fn as_bytes(&self) -> &[u8; 32] {
		self.0.as_bytes()
	}

	/// Creates a unique id from the given u8 value.
	pub fn from_u8(n: u8) -> Self {
		let mut bytes = [0u8; 32];
		bytes[31] = n;
		UniqueId(blake3::Hash::from_bytes(bytes))
	}

	/// Creates a unique id from the given u16 value.
	pub fn from_u16(n: u16) -> Self {
		let mut bytes = [0u8; 32];
		bytes[30..32].copy_from_slice(&n.to_le_bytes());
		UniqueId(blake3::Hash::from_bytes(bytes))
	}

	/// Creates a unique id from the given u32 value.
	pub fn from_u32(n: u32) -> Self {
		let mut bytes = [0u8; 32];
		bytes[28..32].copy_from_slice(&n.to_le_bytes());
		UniqueId(blake3::Hash::from_bytes(bytes))
	}

	/// Creates a unique id from the given u64 value.
	pub fn from_u64(n: u64) -> Self {
		let mut bytes = [0u8; 32];
		bytes[24..32].copy_from_slice(&n.to_le_bytes());
		UniqueId(blake3::Hash::from_bytes(bytes))
	}

	/// Creates a unique id from the given u128 value.
	pub fn from_u128(n: u128) -> Self {
		let mut bytes = [0u8; 32];
		bytes[16..32].copy_from_slice(&n.to_le_bytes());
		UniqueId(blake3::Hash::from_bytes(bytes))
	}

	/// Creates a unique id from the given i8 value.
	pub fn from_i8(n: i8) -> Self {
		let mut bytes = [0u8; 32];
		bytes[31] = n.to_le_bytes()[0];
		UniqueId(blake3::Hash::from_bytes(bytes))
	}

	/// Creates a unique id from the given i16 value.
	pub fn from_i16(n: i16) -> Self {
		let mut bytes = [0u8; 32];
		bytes[30..32].copy_from_slice(&n.to_le_bytes());
		UniqueId(blake3::Hash::from_bytes(bytes))
	}

	/// Creates a unique id from the given i32 value.
	pub fn from_i32(n: i32) -> Self {
		let mut bytes = [0u8; 32];
		bytes[28..32].copy_from_slice(&n.to_le_bytes());
		UniqueId(blake3::Hash::from_bytes(bytes))
	}

	/// Creates a unique id from the given i64 value.
	pub fn from_i64(n: i64) -> Self {
		let mut bytes = [0u8; 32];
		bytes[24..32].copy_from_slice(&n.to_le_bytes());
		UniqueId(blake3::Hash::from_bytes(bytes))
	}

	/// Creates a unique id from the given i128 value.
	pub fn from_i128(n: i128) -> Self {
		let mut bytes = [0u8; 32];
		bytes[16..32].copy_from_slice(&n.to_le_bytes());
		UniqueId(blake3::Hash::from_bytes(bytes))
	}

	/// Creates a unique id from the given usize value.
	pub fn from_usize(n: usize) -> Self {
		let mut bytes = [0u8; 32];
		bytes[32 - std::mem::size_of::<usize>()..32]
			.copy_from_slice(&n.to_le_bytes());
		UniqueId(blake3::Hash::from_bytes(bytes))
	}

	/// Creates a unique id from the given isize value.
	pub fn from_isize(n: isize) -> Self {
		let mut bytes = [0u8; 32];
		bytes[32 - std::mem::size_of::<isize>()..32]
			.copy_from_slice(&n.to_le_bytes());
		UniqueId(blake3::Hash::from_bytes(bytes))
	}

	/// Creates a unique id from the given bytes.
	pub fn from_bytes(bytes: impl Into<[u8; 32]>) -> Self {
		UniqueId(blake3::Hash::from_bytes(bytes.into()))
	}

	/// Generates a random unique id.
	pub fn random() -> Self {
		UniqueId(blake3::Hash::from_bytes(rand::random()))
	}
}
