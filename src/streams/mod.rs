use {
	derive_more::Display,
	serde::{Deserialize, Serialize, de::DeserializeOwned},
};

mod consumer;
mod criteria;
mod error;
mod fanout;
mod producer;
mod protocol;

pub use {
	consumer::Consumer,
	criteria::Criteria,
	error::Error,
	producer::Producer,
};
pub(crate) use {fanout::Fanout, protocol::Protocol};

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
	pub fn of<D: Datum>() -> Self {
		Self(core::any::type_name::<D>().to_string())
	}
}

/// Implemented by all data types that are published to streams.
pub trait Datum:
	PartialEq + Serialize + DeserializeOwned + Send + 'static
{
	fn key(&self) -> &str {
		core::any::type_name_of_val(&self)
	}
}

impl<T> Datum for T where
	T: PartialEq + Serialize + DeserializeOwned + Send + 'static
{
}
