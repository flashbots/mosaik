use {
	core::hash::Hash,
	derive_more::Display,
	serde::{Deserialize, Serialize, de::DeserializeOwned},
};

mod criteria;

pub use criteria::Criteria;

/// Implemented by all data types that are published as streams.
/// This type gives us zero-friction default implementations for
/// any serializable rust type.
pub trait Datum: Serialize + DeserializeOwned + Send + Sync + 'static {
	/// Returns a globally unique stream identifier.
	/// The default implementation uses the rust type name, but in a production
	/// system this should be overridden to provide a stable identifier.
	fn stream_id() -> StreamId {
		StreamId(core::any::type_name::<Self>().to_string())
	}
}

impl<T> Datum for T where T: Serialize + DeserializeOwned + Send + Sync + 'static
{}

/// This type uniquely identifies a stream within the network.
///
/// Notes:
/// - At present, stream ids are derived from the data rust type name.
#[derive(
	Debug,
	Clone,
	Display,
	Serialize,
	Deserialize,
	PartialEq,
	Eq,
	PartialOrd,
	Ord,
	Hash,
)]
pub struct StreamId(String);

impl StreamId {
	/// Create a stream ID for the given data type `D`.
	pub fn of<D: Datum>() -> Self {
		D::stream_id()
	}

	/// Returns true if this stream ID corresponds to data type `D`.
	pub fn is<D: Datum>(&self) -> bool {
		*self == Self::of::<D>()
	}
}
