//! Stream Consumers

use {
	crate::{
		Datum,
		primitives::Short,
		streams::{
			consumer::builder::ConsumerConfig,
			status::{ChannelInfo, Stats, When},
		},
	},
	core::{
		fmt::Debug,
		pin::Pin,
		task::{Context, Poll},
	},
	futures::Stream,
	std::sync::Arc,
	tokio::sync::mpsc,
	tokio_util::sync::DropGuard,
};

mod builder;
mod receiver;
mod worker;

pub use builder::Builder;

/// A local stream consumer handle that allows receiving data from a stream
/// produced by a remote peer.
///
/// Notes:
///
/// - Multiple [`Consumer`] instances can be created for the same stream id and
///   data type `D`. They will each receive their own copy of the data sent to
///   the stream.
///
/// - Each [`Consumer`] instance has its own worker task that manages
///   subscriptions to remote producers, discovery of new producers and
///   receiving data from remote producers.
///
/// - Consumers implement [`Stream`] for receiving datum of type `D`.
pub struct Consumer<D: Datum> {
	/// Allows awaiting changes to the consumer's subscription status.
	status: When,

	/// The consumer-specific configuration as assembled by
	/// `Network::streams().consumer()`. If some configuration values are not
	/// set, they default to the values from `Streams` config.
	config: Arc<ConsumerConfig>,

	/// Aggregated statistics of the consumer.
	stats: Stats,

	/// Channel for receiving datum from the consumer worker task.
	/// Each received datum is paired with its serialized byte length for stats
	/// tracking.
	chan: mpsc::UnboundedReceiver<(D, usize)>,

	/// Drop guard that aborts the consumer worker task when this handle is
	/// dropped.
	_abort: DropGuard,
}

impl<D: Datum> Debug for Consumer<D> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(
			f,
			"Consumer<{}>({}, {} producers)",
			Short(self.config.stream_id),
			std::any::type_name::<D>(),
			self.status.active.borrow().len(),
		)
	}
}

// Public API
impl<D: Datum> Consumer<D> {
	/// Awaits changes to the consumer's status.
	///
	/// example:
	/// ```no_run
	/// let c = net.streams().consume::<MyDatum>();
	///
	/// // resolves when at least one producer is connected
	/// c.when().subscribed().await;
	///
	/// // resolves when at least two producers are connected
	/// c.when().subscribed().to_at_least(2).await;
	/// ```
	pub const fn when(&self) -> &When {
		&self.status
	}

	/// Returns whether the consumer is currently ready to receive data
	/// from other peers.
	pub fn is_online(&self) -> bool {
		self.status.is_online()
	}

	/// Returns the configuration used to create this consumer.
	pub fn config(&self) -> &ConsumerConfig {
		&self.config
	}

	/// Returns the current snapshot of aggregated statistics of the consumer.
	pub const fn stats(&self) -> &Stats {
		&self.stats
	}

	/// Returns an iterator over the currently connected producers for this
	/// consumer. The `PeerEntry` values yielded by the iterator represent the
	/// state of the peers at the time their subscription was established.
	pub fn producers(&self) -> impl Iterator<Item = ChannelInfo> {
		// get current snapshot of active receivers, this clone is cheap
		// because it is an `im::HashMap`, and we want to release the lock
		// on the watch channel as soon as possible.
		let active = self.status.active.borrow().clone();
		active.into_iter().map(|(_, info)| info)
	}
}

impl<D: Datum> Stream for Consumer<D> {
	type Item = D;

	fn poll_next(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Self::Item>> {
		let this = self.get_mut();
		match this.chan.poll_recv(cx) {
			Poll::Ready(Some((datum, bytes_len))) => {
				this.stats.increment_datums();
				this.stats.increment_bytes(bytes_len);
				Poll::Ready(Some(datum))
			}
			Poll::Ready(None) => Poll::Ready(None),
			Poll::Pending => Poll::Pending,
		}
	}
}
