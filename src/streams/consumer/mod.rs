//! Stream Consumers

use super::Datum;

mod builder;
mod status;
mod worker;

pub use {builder::Builder, status::Status};

/// A local stream consumer handle that allows receiving data from a stream
/// produced by a remote peer.
///
/// Notes:
/// - Multiple [`Consumer`] instances can be created for the same stream id and
///   data type `D`. They will each receive their own copy of the data sent to
///   the stream.
pub struct Consumer<D: Datum> {
	_marker: std::marker::PhantomData<D>,
}
