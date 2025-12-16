//! Stream Consumers

use {
	super::{Datum, status::StreamInfo},
	crate::PeerId,
	core::{
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
mod when;
mod worker;

pub use {builder::Builder, when::When};

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
	status: When,
	local_id: PeerId,
	chan: mpsc::UnboundedReceiver<D>,
	_abort: DropGuard,
}

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

	/// Returns an iterator over the currently connected producers for this
	/// consumer. The `PeerEntry` values yielded by the iterator represent the
	/// state of the peers at the time their subscription was established.
	pub fn producers(&self) -> impl Iterator<Item = StreamInfo> {
		// get current snapshot of active receivers, this clone is cheap
		// because it is an `im::HashMap`, and we want to release the lock
		// on the watch channel as soon as possible.
		let receivers = self.status.receivers.borrow().clone();

		receivers.into_iter().map(|(_, handle)| StreamInfo {
			peer: Arc::clone(&handle.peer),
			stats: Arc::clone(&handle.stats),
			producer_id: *handle.peer.id(),
			consumer_id: self.local_id,
			stream_id: D::stream_id(),
			criteria: handle.config.criteria.clone(),
			state: handle.state.clone(),
		})
	}
}

impl<D: Datum> Stream for Consumer<D> {
	type Item = D;

	fn poll_next(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Self::Item>> {
		self.get_mut().chan.poll_recv(cx)
	}
}
