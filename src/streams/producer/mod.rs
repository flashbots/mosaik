//! Stream Producers

use {
	crate::{
		Datum,
		StreamId,
		primitives::Short,
		streams::{
			producer::builder::ProducerConfig,
			status::{ChannelInfo, When},
		},
	},
	core::{
		fmt::Debug,
		pin::{Pin, pin},
		task::{Context, Poll},
	},
	futures::{Sink, SinkExt},
	std::sync::Arc,
	tokio::sync::mpsc::{self, error::TrySendError},
	tokio_util::sync::PollSender,
};

mod builder;
mod error;
mod sender;
mod sink;
mod worker;

/// Internal API
pub(super) use sink::Sinks;
/// Public API
pub use {
	builder::{Builder, Error as BuilderError},
	error::Error,
};

/// A stream producer handle for sending data to remote peers.
///
/// Notes:
///
/// - One [`Network`](crate::Network) can have multiple [`Producer`] instances
///   for the same stream id (mpsc). They all share the same underlying fanout
///   sink.
///
/// - Producers implement [`Sink`] for sending datum of type `D` to the
///   underlying stream.
///
/// - Producers are either online or offline based on their publishing
///   conditions as specified in the builder or configuration. When offline,
///   attempts to send data will fail. Online conditions can be observed through
///   the [`when()`](Producer::when) API. They can be things like minimum number
///   of subscribers, required tags, or custom predicates on the current set of
///   subscribers. Online conditions are re-evaluated whenever there is a change
///   in the set of active subscribers. Online conditions can be configured via
///   the [`network.streams().producer().online_when(..)`] API. By default,
///   producers are online when they have at least one active consumer.
///
/// - Using `Sink::send` on a producer will first wait for the producer to be
///   online before attempting to send the datum to the underlying channel. If
///   the producer is offline, the `Sink::poll_ready` call will not complete
///   until the producer becomes online. Alternatively, the `try_send` method
///   can be used to attempt to send a datum immediately, returning an error if
///   the producer is offline or if the underlying channel is not ready to
///   accept new datum.
///
/// - Each connected consumer has its own independent sender loop that is
///   responsible for delivering datums to that consumer over transport. This
///   design is chosen to avoid stalling the entire producer when one of the
///   consumers is slow or unresponsive.
#[derive(Clone)]
pub struct Producer<D: Datum> {
	/// Allows awaiting changes to the producer's subscriptions status.
	status: When,

	/// Channel for sending datum to the underlying producer sink.
	chan: PollSender<D>,

	/// The producer-specific configuration as assembled by
	/// `Network::streams().producer()`. If some configuration values are not
	/// set, they default to the values from `Streams` config.
	config: Arc<ProducerConfig>,
}

impl<D: Datum> Debug for Producer<D> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(
			f,
			"Producer<{}>({})",
			Short(self.config.stream_id),
			std::any::type_name::<D>()
		)
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

	/// Returns whether the producer is has its publishing conditions met and is
	/// ready to send data to connected consumers.
	pub fn is_online(&self) -> bool {
		self.status.is_online()
	}

	/// Attempts to send a datum to the underlying stream producer sink.
	pub fn try_send(&self, item: D) -> Result<(), Error<D>> {
		if !self.is_online() {
			return Err(Error::Offline(item));
		}

		let Some(inner) = self.chan.get_ref() else {
			return Err(Error::Closed(Some(item)));
		};

		inner.try_send(item).map_err(|e| match e {
			TrySendError::Full(d) => Error::Full(d),
			TrySendError::Closed(d) => Error::Closed(Some(d)),
		})
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

/// Sink implementation for sending datum to remote consumers.
///
/// Notes:
/// - The sink will only accept datum to send when the producer is online.
///   Otherwise an `Error::Offline` is returned.
/// - The sink may return `Error::Closed` if the underlying sink is closed.
/// - When sending datums, the sink will be in a ready state only when the
///   producer is online and the underlying channel is ready to accept new
///   datum, otherwise `Sink::send` will not resolve until both conditions are
///   met.
impl<D: Datum> Sink<D> for Producer<D> {
	type Error = Error<D>;

	fn poll_ready(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Result<(), Self::Error>> {
		let this = self.get_mut();
		let mut online_fut = pin!(this.status.online.wait_for(|s| *s));

		match Pin::new(&mut online_fut).poll(cx) {
			Poll::Ready(_) => {}
			Poll::Pending => return Poll::Pending,
		}

		this
			.chan
			.poll_ready_unpin(cx)
			.map_err(|e| Error::Closed(e.into_inner()))
	}

	fn start_send(self: Pin<&mut Self>, item: D) -> Result<(), Self::Error> {
		self.try_send(item)
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
