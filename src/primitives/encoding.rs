//! Library-level centralized encoding and decoding utilities that
//! define the wire-format of all exchanged messages across the SDK.
//!
//! Currently uses `postcard` as the underlying serialization format.

use {
	bytes::Bytes,
	serde::{Serialize, de::DeserializeOwned},
};

pub type EncodeError = postcard::Error;
pub type DecodeError = postcard::Error;

#[track_caller]
pub(crate) fn serialize<T: Serialize>(value: &T) -> Bytes {
	postcard::to_allocvec(value)
		.expect("serialization should never fail")
		.into()
}

pub fn try_serialize<T: Serialize>(value: &T) -> Result<Bytes, EncodeError> {
	postcard::to_allocvec(value).map(Bytes::from)
}

#[track_caller]
pub(crate) fn serialize_to_writer<T: Serialize>(
	value: &T,
	writer: &mut impl std::io::Write,
) {
	postcard::to_io(value, writer).expect("serialization should never fail");
}

pub fn try_serialize_to_writer<T: Serialize>(
	value: &T,
	writer: &mut impl std::io::Write,
) -> Result<(), EncodeError> {
	postcard::to_io(value, writer).map(|_| ())
}

pub fn deserialize<T: DeserializeOwned>(
	bytes: impl AsRef<[u8]>,
) -> Result<T, DecodeError> {
	postcard::from_bytes(bytes.as_ref())
}
