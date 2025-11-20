use {
	derive_more::Display,
	serde::{Deserialize, Serialize, de::DeserializeOwned},
};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Criteria {}

impl Criteria {
	pub fn matches<D: Datum>(&self, _item: &D) -> bool {
		true
	}
}

/// Implemented by all data types that are published to streams.
pub trait Datum:
	core::fmt::Debug + Serialize + DeserializeOwned + Send + Sync + 'static
{
	fn stream_id(&self) -> &str {
		core::any::type_name_of_val(&self)
	}
}

impl<T> Datum for T where
	T: core::fmt::Debug + Serialize + DeserializeOwned + Send + Sync + 'static
{
}

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
		Self(core::any::type_name::<D>().to_string())
	}

	/// Returns true if this stream ID corresponds to data type `D`.
	pub fn is<D: Datum>(&self) -> bool {
		*self == Self::of::<D>()
	}
}
