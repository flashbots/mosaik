use {
	crate::StreamId,
	bytes::Bytes,
	derive_more::{Deref, DerefMut},
	serde::{Deserializer, Serialize, Serializer, de::DeserializeOwned},
};

/// Implemented by all data types that are published as streams.
///
/// This type gives us zero-friction default implementations for
/// any serializable rust type.
pub trait Datum: Sized + Send + 'static {
	type EncodeError: core::error::Error + Send + Sync + 'static;
	type DecodeError: core::error::Error + Send + Sync + 'static;

	/// Returns the default stream id derived from the datum type name.
	/// This is the stream id used if no custom stream id is provided when
	/// building producers or consumers for this datum type.
	fn derived_stream_id() -> StreamId {
		core::any::type_name::<Self>().into()
	}

	/// Serializes the datum into bytes for sending over the network.
	fn encode(&self) -> Result<Bytes, Self::EncodeError>;

	/// Deserializes the datum from bytes received over the network.
	fn decode(bytes: &[u8]) -> Result<Self, Self::DecodeError>;
}

/// Blanket implementation of `Datum` for any type that implements `Serialize`
/// and `DeserializeOwned` using `postcard` as the underlying serialization
/// format.
impl<T> Datum for T
where
	T: Serialize + DeserializeOwned + Send + 'static,
{
	type DecodeError = postcard::Error;
	type EncodeError = postcard::Error;

	fn encode(&self) -> Result<Bytes, Self::EncodeError> {
		crate::primitives::encoding::try_serialize(&self)
	}

	fn decode(bytes: &[u8]) -> Result<Self, Self::DecodeError> {
		crate::primitives::encoding::deserialize(bytes)
	}
}

/// A serde-compatible wrapper that serializes/deserializes a `Datum` as opaque
/// bytes.
///
/// Use this to wrap `Datum` types in structs that derive
/// `Serialize`/`Deserialize`, so that standard serde containers (`Vec`, tuples,
/// etc.) work automatically.
#[derive(Clone, Debug, Deref, DerefMut, PartialEq, Eq, Hash)]
pub struct Encoded<T>(pub T);

impl<T: Datum> From<T> for Encoded<T> {
	fn from(value: T) -> Self {
		Self(value)
	}
}

impl<T: Datum> Serialize for Encoded<T> {
	fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		let encoded = self.0.encode().map_err(serde::ser::Error::custom)?;
		encoded.serialize(serializer)
	}
}

impl<'de, T: Datum> serde::Deserialize<'de> for Encoded<T> {
	fn deserialize<D: Deserializer<'de>>(
		deserializer: D,
	) -> Result<Self, D::Error> {
		let bytes = Vec::<u8>::deserialize(deserializer)?;
		T::decode(&bytes)
			.map(Encoded)
			.map_err(serde::de::Error::custom)
	}
}
