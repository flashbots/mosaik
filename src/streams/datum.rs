use {
	crate::StreamId,
	serde::{Serialize, de::DeserializeOwned},
};

/// Implemented by all data types that are published as streams.
/// This type gives us zero-friction default implementations for
/// any serializable rust type.
pub trait Datum: Serialize + DeserializeOwned + Send + Sync + 'static {
	/// Returns a globally unique stream identifier.
	/// The default implementation uses the rust type name, but in a production
	/// system this should be overridden to provide a stable identifier.
	fn stream_id() -> StreamId {
		core::any::type_name::<Self>().into()
	}
}

impl<T> Datum for T where T: Serialize + DeserializeOwned + Send + Sync + 'static
{}
