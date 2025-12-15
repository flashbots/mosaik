//! Stream Producers

use {
	crate::{Datum, StreamId},
	core::{
		fmt::Debug,
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
mod when;
mod worker;

/// Internal API
pub(super) use sink::Sinks;
/// Public API
pub use {
	builder::{Builder, Error as BuilderError},
	error::{Error, ProducerError},
	when::When,
};

/// a local stream producer handle for sending data to remote peers.
///
/// Notes:
///
/// - One [`Network`](crate::Network) can have multiple [`Producer`] instances
///   for the same stream id (mpsc). They all share the same underlying fanout
///   sink.
///
/// - Producers implement [`Sink`] for sending datum of type `D` to the
///   underlying stream.
#[derive(Clone)]
pub struct Producer<D: Datum> {
	status: When,
	chan: PollSender<D>,
}

impl<D: Datum> Debug for Producer<D> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "Producer<{}>", D::stream_id())
	}
}

impl<D: Datum> Producer<D> {
	pub(crate) fn new(chan: mpsc::Sender<D>, status: When) -> Self {
		Self {
			status,
			chan: PollSender::new(chan),
		}
	}

	/// The stream id associated with this producer.
	pub fn stream_id(&self) -> StreamId {
		D::stream_id()
	}

	/// Access to the producer's status information.
	pub const fn when(&self) -> &When {
		&self.status
	}
}

impl<D: Datum> Sink<D> for Producer<D> {
	type Error = ProducerError<D>;

	fn poll_ready(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Result<(), Self::Error>> {
		self
			.get_mut()
			.chan
			.poll_ready_unpin(cx)
			.map_err(|e| ProducerError::Closed(e.into_inner()))
	}

	fn start_send(self: Pin<&mut Self>, item: D) -> Result<(), Self::Error> {
		self
			.get_mut()
			.chan
			.start_send_unpin(item)
			.map_err(|e| ProducerError::Closed(e.into_inner()))
	}

	fn poll_flush(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Result<(), Self::Error>> {
		self
			.get_mut()
			.chan
			.poll_flush_unpin(cx)
			.map_err(|e| ProducerError::Closed(e.into_inner()))
	}

	fn poll_close(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Result<(), Self::Error>> {
		self
			.get_mut()
			.chan
			.poll_close_unpin(cx)
			.map_err(|e| ProducerError::Closed(e.into_inner()))
	}
}
