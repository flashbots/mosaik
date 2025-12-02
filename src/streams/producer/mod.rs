//! Stream Producers

use {
	crate::Datum,
	core::{
		pin::Pin,
		task::{Context, Poll},
	},
	futures::{Sink, SinkExt},
	tokio::sync::mpsc,
	tokio_util::sync::PollSender,
};

mod builder;
mod error;
mod sink;
mod status;
mod worker;

/// Internal API
pub(super) use sink::Sinks;
/// Public API
pub use {
	builder::Builder,
	error::{Error, SendError},
	status::Status,
};

/// Producer handle for sending data to a stream.
///
/// Notes:
///
/// - One [`Network`](crate::Network) can have multiple [`Producer`] instances
///   for the same stream id (mpsc). They all share the same underlying fanout
///   sink.
///
/// - Producers implement [`Sink`] for sending datum of type `D` to the
///   underlying stream.
pub struct Producer<D: Datum> {
	status: Status,
	chan: PollSender<D>,
}

impl<D: Datum> Producer<D> {
	pub(crate) fn new(chan: mpsc::Sender<D>, status: Status) -> Self {
		Self {
			status,
			chan: PollSender::new(chan),
		}
	}

	/// Access to the producer's status information.
	pub const fn status(&self) -> &Status {
		&self.status
	}
}

impl<D: Datum> Sink<D> for Producer<D> {
	type Error = SendError<D>;

	fn poll_ready(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Result<(), Self::Error>> {
		self
			.get_mut()
			.chan
			.poll_ready_unpin(cx)
			.map_err(|e| SendError::Closed(e.into_inner()))
	}

	fn start_send(self: Pin<&mut Self>, item: D) -> Result<(), Self::Error> {
		self
			.get_mut()
			.chan
			.start_send_unpin(item)
			.map_err(|e| SendError::Closed(e.into_inner()))
	}

	fn poll_flush(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Result<(), Self::Error>> {
		self
			.get_mut()
			.chan
			.poll_flush_unpin(cx)
			.map_err(|e| SendError::Closed(e.into_inner()))
	}

	fn poll_close(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Result<(), Self::Error>> {
		self
			.get_mut()
			.chan
			.poll_close_unpin(cx)
			.map_err(|e| SendError::Closed(e.into_inner()))
	}
}
