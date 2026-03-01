//! Library-level centralized encoding and decoding utilities that
//! define the wire-format of all exchanged messages across the SDK.
//!
//! Currently uses `postcard` as the underlying serialization format.

use {
	bytes::Bytes,
	serde::{Serialize, de::DeserializeOwned},
};

pub fn serialize<T: Serialize>(value: &T) -> Bytes {
	postcard::to_allocvec(value)
		.expect("serialization should never fail")
		.into()
}

pub fn try_serialize<T: Serialize>(
	value: &T,
) -> Result<Bytes, postcard::Error> {
	postcard::to_allocvec(value).map(Bytes::from)
}

pub fn serialize_to_writer<T: Serialize>(
	value: &T,
	writer: impl std::io::Write,
) {
	postcard::to_io(value, writer).expect("serialization should never fail");
}

pub fn deserialize<T: DeserializeOwned>(
	bytes: impl AsRef<[u8]>,
) -> Result<T, postcard::Error> {
	postcard::from_bytes(bytes.as_ref())
}
