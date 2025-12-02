use super::Datum;

mod builder;
mod status;
mod worker;

/// Public API
pub use {builder::Builder, status::Status};

/// A local stream consumer handle that allows receiving data from a stream
/// produced by a remote peer.
pub struct Consumer<D: Datum> {
	_marker: std::marker::PhantomData<D>,
}
