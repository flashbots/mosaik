//! Stream Consumers

use {
	super::Datum,
	crate::{
		discovery::PeerEntry,
		primitives::Short,
		streams::consumer::receiver::ReceiverHandle,
	},
	core::{
		fmt,
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

pub use {
	builder::Builder,
	receiver::{State, Stats},
	when::When,
};

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
pub struct Consumer<D> {
	status: When,
	chan: mpsc::UnboundedReceiver<D>,
	_abort: DropGuard,
}

impl<D> Consumer<D> {
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
	pub fn producers(&self) -> impl Iterator<Item = Subscription> {
		// get current snapshot of active receivers, this clone is cheap
		// because it is an `im::HashMap`.
		let receivers = self.status.receivers.borrow().clone();

		receivers
			.into_iter()
			.map(|(_, handle)| Subscription(Arc::clone(&handle)))
	}
}

impl<D> Stream for Consumer<D> {
	type Item = D;

	fn poll_next(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Self::Item>> {
		self.get_mut().chan.poll_recv(cx)
	}
}

/// A handle representing an active subscription to a producer from a specific
/// consumer instance.
///
/// This is used to inspect the state and stats of the subscription.
#[derive(Clone)]
pub struct Subscription(Arc<ReceiverHandle>);

impl Subscription {
	/// Returns the peer entry of the producer this subscription is connected to.
	///
	/// The state of the `PeerEntry` reflects the state of the producer at the
	/// time the subscription was established.
	pub fn peer(&self) -> &PeerEntry {
		self.0.peer()
	}

	/// Returns the current state of the connection.
	pub fn state(&self) -> State {
		*self.0.state().borrow()
	}

	/// Returns the current snapshot of statistics of the subscription.
	pub fn stats(&self) -> &Stats {
		self.0.stats()
	}
}

impl fmt::Debug for Subscription {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("Subscription")
			.field("peer", &Short(self.0.peer()).to_string())
			.field("state", &self.0.state().borrow())
			.field("stats", &self.0.stats())
			.finish()
	}
}
