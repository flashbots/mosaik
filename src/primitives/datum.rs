use {
	crate::StreamId,
	bytes::Bytes,
	serde::{Serialize, de::DeserializeOwned},
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
