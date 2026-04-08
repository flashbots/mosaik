use {
	crate::{
		NetworkId,
		PeerId,
		primitives::{Expiration, InvalidTicket},
	},
	lsmtree::{BadProof, KVStore},
	serde::{Deserialize, Serialize},
	std::collections::BTreeMap,
};

/// A 48-byte TDX measurement register value (MRTD, RTMR0–3).
///
/// Used to express an expected measurement when configuring a
/// [`TdxValidator`].
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Measurement([u8; 48]);

impl Measurement {
	/// Creates a measurement from a 48-byte array.
	pub const fn new(bytes: [u8; 48]) -> Self {
		Self(bytes)
	}

	/// Creates a measurement from a 96-character hex string.
	///
	/// # Panics
	/// Panics if the string is not exactly 96 hex characters (48 bytes).
	pub const fn hex(input: &str) -> Self {
		const fn hex_nibble(b: u8) -> u8 {
			match b {
				b'0'..=b'9' => b - b'0',
				b'a'..=b'f' => b - b'a' + 10,
				b'A'..=b'F' => b - b'A' + 10,
				_ => panic!("Invalid hex character"),
			}
		}

		assert!(
			input.len() == 96,
			"Hex string must be exactly 96 characters (48 bytes)"
		);

		let bytes = input.as_bytes();

		let mut arr = [0u8; 48];

		let mut i = 0;
		while i < 48 {
			let hi = hex_nibble(bytes[i * 2]);
			let lo = hex_nibble(bytes[i * 2 + 1]);
			arr[i] = (hi << 4) | lo;
			i += 1;
		}

		Self(arr)
	}

	pub const fn as_bytes(&self) -> &[u8; 48] {
		&self.0
	}
}

impl TryFrom<&[u8]> for Measurement {
	type Error = InvalidTicket;

	fn try_from(bytes: &[u8]) -> Result<Self, InvalidTicket> {
		bytes.try_into().map(Self).map_err(|_| InvalidTicket)
	}
}

impl From<[u8; 48]> for Measurement {
	fn from(bytes: [u8; 48]) -> Self {
		Self(bytes)
	}
}

impl core::fmt::Debug for Measurement {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		write!(f, "Measurement({})", hex::encode(self.0))
	}
}

impl core::fmt::Display for Measurement {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		write!(f, "{}", hex::encode(self.0))
	}
}

#[derive(Serialize, Deserialize)]
pub struct TdxTicket {
	pub quote: Vec<u8>,
	pub system: SystemData,
	pub user: UserData,
}

#[derive(Serialize, Deserialize)]
pub struct UserData(BTreeMap<Vec<u8>, Vec<u8>>);

impl KVStore for UserData {
	type Error = BadProof;
	type Hasher = blake3::Hasher;

	fn get(&self, key: &[u8]) -> Result<Option<bytes::Bytes>, Self::Error> {
		todo!()
	}

	fn set(
		&mut self,
		key: bytes::Bytes,
		value: bytes::Bytes,
	) -> Result<(), Self::Error> {
		todo!()
	}

	fn remove(&mut self, key: &[u8]) -> Result<bytes::Bytes, Self::Error> {
		todo!()
	}

	fn contains(&self, key: &[u8]) -> Result<bool, Self::Error> {
		todo!()
	}
}

#[derive(Serialize, Deserialize)]
pub struct SystemData {
	pub peer_id: PeerId,
	pub network_id: NetworkId,
	pub expires_at: Expiration,
}
