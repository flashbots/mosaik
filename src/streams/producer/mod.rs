use {crate::Datum, tokio::sync::mpsc};

mod builder;
mod error;
mod sink;
mod status;
mod worker;

/// Internal API
pub(super) use sink::Sinks;
/// Public API
pub use {builder::Builder, error::Error, status::Status};

/// Producer handle for sending data to a stream.
///
/// Notes:
///  - One [`crate::Network`] can have multiple [`Producer`] instances for the
///    same data type `D` that share the same stream id.
pub struct Producer<D: Datum> {
	_chan: mpsc::Sender<D>,
	_status: Status,
}

impl<D: Datum> Producer<D> {
	pub(crate) fn new(chan: mpsc::Sender<D>, status: Status) -> Self {
		Self {
			_chan: chan,
			_status: status,
		}
	}
}
