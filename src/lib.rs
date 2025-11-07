mod error;
mod id;
mod local;
mod network;
mod peer;
mod streams;

pub mod prelude {
	pub use super::{
		error::Error,
		id::NetworkId,
		network::*,
		streams::{Consumer, Datum, Producer},
	};
}

#[cfg(feature = "test-utils")]
pub mod test_utils;
