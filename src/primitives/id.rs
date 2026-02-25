use {
	crate::primitives::Short,
	core::{convert::Infallible, fmt, str::FromStr},
	derive_more::Deref,
	serde::{Deserialize, Deserializer, Serialize, de},
};

/// Unique 32-byte identifier used throughout the Mosaik ecosystem.
pub type UniqueId = Digest;

/// A tag is an opaque 32-byte hash that can be used to label peers, networks,
/// resources and other things. Its use is context dependent.
pub type Tag = UniqueId;

/// This type is used to uniquely identify various things in the Mosaik
/// ecosystem such as streams, peers, and networks.
///
/// It is represented as a 32-byte array, typically derived from a Blake3 hash
/// of some preimage. In some cases it can also carry additional semantics, such
/// as being derived from a secret for group keys or being a hash of a peer's
/// public key for peer ids.
///
/// All unique ids in Mosaik can be derived from strings and byte slices using
/// the `From` trait implementations provided.
///
/// Notes:
///  - when serialized to human readable formats (e.g., JSON), `UniqueId`s are
///    represented as hex-encoded strings.
///  - when serialized to binary formats (e.g., postcard), `UniqueId`s are
///    represented as raw 32-byte arrays.
#[derive(Clone, Copy, Deref)]
pub struct Digest(blake3::Hash);

impl<T: AsRef<str>> From<T> for Digest {
	fn from(s: T) -> Self {
		let s = s.as_ref();
		// if the string is already a 32-byte hex string, decode it directly
		// otherwise, hash it to produce the unique id
		match hex::decode(s) {
			Ok(b) if b.len() == 32 => {
				Self(blake3::Hash::from_slice(&b).expect("slice is 32 bytes"))
			}
			_ => Self(blake3::hash(s.as_bytes())),
		}
	}
}

impl PartialEq for Digest {
	fn eq(&self, other: &Self) -> bool {
		self.0 == other.0
	}
}
impl Eq for Digest {}

impl PartialOrd for Digest {
	fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
		Some(self.cmp(other))
	}
}

impl Ord for Digest {
	fn cmp(&self, other: &Self) -> core::cmp::Ordering {
		self.0.as_bytes().cmp(other.0.as_bytes())
	}
}

impl core::hash::Hash for Digest {
	fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
		self.0.as_bytes().hash(state);
	}
}

impl AsRef<[u8]> for Digest {
	fn as_ref(&self) -> &[u8] {
		self.0.as_bytes()
	}
}

impl From<Digest> for [u8; 32] {
	fn from(id: Digest) -> Self {
		*id.0.as_bytes()
	}
}

impl From<&Digest> for [u8; 32] {
	fn from(id: &Digest) -> Self {
		*id.0.as_bytes()
	}
}

impl PartialEq<&Self> for Digest {
	fn eq(&self, other: &&Self) -> bool {
		self.0 == other.0
	}
}

impl PartialEq<Digest> for &Digest {
	fn eq(&self, other: &Digest) -> bool {
		self.0 == other.0
	}
}

impl FromStr for Digest {
	type Err = Infallible;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		Ok(Self::from(s))
	}
}

impl fmt::Debug for Digest {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}", self.0.to_hex())
	}
}

impl fmt::Display for Digest {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}", Short(self.0.as_bytes()))
	}
}

impl Serialize for Digest {
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

impl<'de> Deserialize<'de> for Digest {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: Deserializer<'de>,
	{
		if deserializer.is_human_readable() {
			let s = String::deserialize(deserializer)?;
			Ok(Self(blake3::Hash::from_hex(&s).map_err(de::Error::custom)?))
		} else {
			let bytes = <[u8; 32]>::deserialize(deserializer)?;
			Ok(Self(blake3::Hash::from_bytes(bytes)))
		}
	}
}

impl Digest {
	/// Returns a unique id consisting of 32 zero bytes.
	pub const fn zero() -> Self {
		Self(blake3::Hash::from_bytes([0u8; 32]))
	}

	/// Returns the byte representation of the unique id.
	pub const fn as_bytes(&self) -> &[u8; 32] {
		self.0.as_bytes()
	}

	/// Creates a unique id from the given blake3 hasher by finalizing it.
	pub fn from_hasher(hasher: &blake3::Hasher) -> Self {
		Self(hasher.finalize())
	}

	/// Creates a unique id by hashing the concatenation of the given parts.
	pub fn from_parts(parts: &[impl AsRef<[u8]>]) -> Self {
		let mut hasher = blake3::Hasher::new();
		for part in parts {
			hasher.update(part.as_ref());
		}
		Self::from_hasher(&hasher)
	}

	/// Creates a unique id from the given u8 value.
	pub const fn from_u8(n: u8) -> Self {
		let mut bytes = [0u8; 32];
		bytes[31] = n;
		Self(blake3::Hash::from_bytes(bytes))
	}

	/// Creates a unique id from the given u16 value.
	pub fn from_u16(n: u16) -> Self {
		let mut bytes = [0u8; 32];
		bytes[30..32].copy_from_slice(&n.to_le_bytes());
		Self(blake3::Hash::from_bytes(bytes))
	}

	/// Creates a unique id from the given u32 value.
	pub fn from_u32(n: u32) -> Self {
		let mut bytes = [0u8; 32];
		bytes[28..32].copy_from_slice(&n.to_le_bytes());
		Self(blake3::Hash::from_bytes(bytes))
	}

	/// Creates a unique id from the given u64 value.
	pub fn from_u64(n: u64) -> Self {
		let mut bytes = [0u8; 32];
		bytes[24..32].copy_from_slice(&n.to_le_bytes());
		Self(blake3::Hash::from_bytes(bytes))
	}

	/// Creates a unique id from the given u128 value.
	pub fn from_u128(n: u128) -> Self {
		let mut bytes = [0u8; 32];
		bytes[16..32].copy_from_slice(&n.to_le_bytes());
		Self(blake3::Hash::from_bytes(bytes))
	}

	/// Creates a unique id from the given i8 value.
	pub const fn from_i8(n: i8) -> Self {
		let mut bytes = [0u8; 32];
		bytes[31] = n.to_le_bytes()[0];
		Self(blake3::Hash::from_bytes(bytes))
	}

	/// Creates a unique id from the given i16 value.
	pub fn from_i16(n: i16) -> Self {
		let mut bytes = [0u8; 32];
		bytes[30..32].copy_from_slice(&n.to_le_bytes());
		Self(blake3::Hash::from_bytes(bytes))
	}

	/// Creates a unique id from the given i32 value.
	pub fn from_i32(n: i32) -> Self {
		let mut bytes = [0u8; 32];
		bytes[28..32].copy_from_slice(&n.to_le_bytes());
		Self(blake3::Hash::from_bytes(bytes))
	}

	/// Creates a unique id from the given i64 value.
	pub fn from_i64(n: i64) -> Self {
		let mut bytes = [0u8; 32];
		bytes[24..32].copy_from_slice(&n.to_le_bytes());
		Self(blake3::Hash::from_bytes(bytes))
	}

	/// Creates a unique id from the given i128 value.
	pub fn from_i128(n: i128) -> Self {
		let mut bytes = [0u8; 32];
		bytes[16..32].copy_from_slice(&n.to_le_bytes());
		Self(blake3::Hash::from_bytes(bytes))
	}

	/// Creates a unique id from the given usize value.
	pub fn from_usize(n: usize) -> Self {
		let mut bytes = [0u8; 32];
		bytes[32 - std::mem::size_of::<usize>()..32]
			.copy_from_slice(&n.to_le_bytes());
		Self(blake3::Hash::from_bytes(bytes))
	}

	/// Creates a unique id from the given isize value.
	pub fn from_isize(n: isize) -> Self {
		let mut bytes = [0u8; 32];
		bytes[32 - std::mem::size_of::<isize>()..32]
			.copy_from_slice(&n.to_le_bytes());
		Self(blake3::Hash::from_bytes(bytes))
	}

	/// Creates a unique id from the given bytes.
	pub const fn from_bytes(bytes: [u8; 32]) -> Self {
		Self(blake3::Hash::from_bytes(bytes))
	}

	/// Generates a random unique id.
	pub fn random() -> Self {
		Self(blake3::Hash::from_bytes(rand::random()))
	}

	/// Derives a new unique id from this one using the given info as
	/// context. This is used when you want to create a new unique id that is
	/// deterministically related to an existing one, such as replay streams for
	/// streams.
	#[must_use]
	pub fn derive(&self, info: impl AsRef<[u8]>) -> Self {
		let mut hasher = blake3::Hasher::new();
		hasher.update(self.as_bytes());
		hasher.update(info.as_ref());
		Self(hasher.finalize())
	}

	/// Returns a new digest that is the hash of this digest.
	#[must_use]
	pub fn hashed(&self) -> Self {
		let mut hasher = blake3::Hasher::new();
		hasher.update(self.as_bytes());
		Self::from_hasher(&hasher)
	}
}

/// Converts a string literal into a `UniqueId` at compile time.
///
/// This macro accepts either:
/// - A 64-character hex string (representing 32 bytes), which is decoded
///   directly.
/// - Any other string, which is hashed using blake3 to produce the id (same
///   behavior as `UniqueId::from`).
///
/// # Examples
///
/// ```
/// use mosaik::unique_id;
///
/// // From a hex string:
/// const HEX_ID: mosaik::UniqueId = unique_id!(
/// 	"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
/// );
///
/// // From an arbitrary string (hashed with blake3):
/// const NAMED_ID: mosaik::UniqueId = unique_id!("my-stream-name");
/// ```
#[macro_export]
macro_rules! unique_id {
	($s:expr) => {
		$crate::primitives::UniqueId::from_bytes($crate::__unique_id_impl!($s))
	};
}
