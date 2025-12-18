use {
	crate::StreamId,
	serde::{Serialize, de::DeserializeOwned},
};

/// Implemented by all data types that are published as streams.
///
/// This type gives us zero-friction default implementations for
/// any serializable rust type.
pub trait Datum: Serialize + DeserializeOwned + Send + Sync + 'static {
	/// Returns the default stream id derived from the datum type name.
	/// This is the stream id used if no custom stream id is provided when
	/// building producers or consumers for this datum type.
	fn derived_stream_id() -> StreamId {
		core::any::type_name::<Self>().into()
	}
}

impl<T> Datum for T where T: Serialize + DeserializeOwned + Send + Sync + 'static
{}
