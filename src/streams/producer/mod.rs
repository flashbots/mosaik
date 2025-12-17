//! Stream Producers

use {
	super::{
		Datum,
		StreamId,
		status::{ChannelInfo, When},
	},
	builder::ProducerConfig,
	core::{
		fmt::Debug,
		pin::Pin,
		task::{Context, Poll},
	},
	futures::{Sink, SinkExt},
	std::sync::Arc,
	tokio::sync::mpsc,
	tokio_util::sync::PollSender,
};

mod builder;
mod error;
mod sink;
mod worker;

/// Internal API
pub(super) use sink::Sinks;
/// Public API
pub use {
	builder::{Builder, Error as BuilderError},
	error::Error,
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
	config: Arc<ProducerConfig>,
}

impl<D: Datum> Debug for Producer<D> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "Producer<{}>", D::stream_id())
	}
}

impl<D: Datum> Producer<D> {
	pub(crate) fn new(
		chan: mpsc::Sender<D>,
		status: When,
		config: Arc<ProducerConfig>,
	) -> Self {
		Self {
			status,
			config,
			chan: PollSender::new(chan),
		}
	}

	/// The stream id associated with this producer.
	pub fn stream_id(&self) -> &StreamId {
		&self.config().stream_id
	}

	/// Awaits changes to the producer's status.
	///
	/// example:
	/// ```no_run
	/// let p = net.streams().produce::<MyDatum>();
	/// // resolves when the producer is ready to interact with other peers
	/// p.when().online().await;
	/// // resolves when at least one subscriber is connected
	/// p.when().subscribed().await;
	/// // resolves when at least two subscribers are connected
	/// p.when().subscribed().by_at_least(2).await;
	/// ```
	pub const fn when(&self) -> &When {
		&self.status
	}

	/// Returns the configuration used to create this producer.
	pub fn config(&self) -> &ProducerConfig {
		&self.config
	}

	/// Returns an iterator over the active subscriptions to this producer.
	///
	/// Each subscription represents an active connection from a remote
	/// consumer that is receiving data from this producer. The peer entry
	/// stored here reflects the state of the consumer at the time the
	/// subscription was established.
	pub fn consumers(&self) -> impl Iterator<Item = ChannelInfo> {
		// get the latest snapshot of active subscriptions info
		// and release lock on the watch channel as soon as possible
		let active = self.status.active.borrow().clone();

		active.into_iter().map(|(_, info)| info)
	}
}

impl<D: Datum> Sink<D> for Producer<D> {
	type Error = Error<D>;

	fn poll_ready(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Result<(), Self::Error>> {
		self
			.get_mut()
			.chan
			.poll_ready_unpin(cx)
			.map_err(|e| Error::Closed(e.into_inner()))
	}

	fn start_send(self: Pin<&mut Self>, item: D) -> Result<(), Self::Error> {
		self
			.get_mut()
			.chan
			.start_send_unpin(item)
			.map_err(|e| Error::Closed(e.into_inner()))
	}

	fn poll_flush(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Result<(), Self::Error>> {
		self
			.get_mut()
			.chan
			.poll_flush_unpin(cx)
			.map_err(|e| Error::Closed(e.into_inner()))
	}

	fn poll_close(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Result<(), Self::Error>> {
		self
			.get_mut()
			.chan
			.poll_close_unpin(cx)
			.map_err(|e| Error::Closed(e.into_inner()))
	}
}
