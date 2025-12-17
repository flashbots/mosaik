//! Stream Consumers

use {
	super::{
		Datum,
		status::{ChannelInfo, When},
	},
	core::{
		pin::Pin,
		task::{Context, Poll},
	},
	futures::Stream,
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
	status: When,
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

	/// Returns whether the consumer is currently ready to receive data
	/// from other peers.
	pub fn is_online(&self) -> bool {
		self.status.is_online()
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
		self.get_mut().chan.poll_recv(cx)
	}
}
