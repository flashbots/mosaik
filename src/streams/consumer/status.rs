use {
	core::sync::atomic::{AtomicU32, AtomicU64, Ordering},
	tokio::sync::Notify,
	tokio_util::sync::CancellationToken,
};

/// This type provides status information and updates about a local stream
/// consumer.
#[derive(Debug, Default)]
pub struct Status {
	/// The number of datums received by this consumer.
	pub(crate) items_received: AtomicU64,

	/// The number of bytes received by this consumer.
	pub(crate) bytes_received: AtomicU64,

	/// The current number of connected remote producers for this consumer.
	pub(crate) producers_count: AtomicU32,

	/// Receives notifications when important status changes occur.
	/// Currently it is notified when the number of connected producers changes.
	pub(crate) notify: Notify,

	/// Keeps track of this stream consumer termination.
	pub(crate) cancel: CancellationToken,
}

impl Status {
	pub(crate) fn new() -> Self {
		Self {
			items_received: AtomicU64::new(0),
			bytes_received: AtomicU64::new(0),
			producers_count: AtomicU32::new(0),
			notify: Notify::new(),
			cancel: CancellationToken::new(),
		}
	}
}

impl Status {
	/// Returns the current number of connected remote producers for this
	/// consumer.
	pub fn producers_count(&self) -> u32 {
		self.producers_count.load(Ordering::Relaxed)
	}

	/// Returns true if this consumer is currently subscribed to at least one
	/// producer.
	pub fn is_subscribed(&self) -> bool {
		self.producers_count() > 0
	}

	/// Returns true if this consumer has been terminated.
	/// When terminated, the consumer can no longer receive data.
	pub fn is_terminated(&self) -> bool {
		self.cancel.is_cancelled()
	}

	/// Returns the cumulative number of datums received by this consumer.
	pub fn items_received(&self) -> u64 {
		self.items_received.load(Ordering::Relaxed)
	}

	/// Returns the cumulative number of bytes received by this consumer.
	pub fn bytes_received(&self) -> u64 {
		self.bytes_received.load(Ordering::Relaxed)
	}

	/// Awaits until this consumer is subscribed to at least one producer.
	/// Returns a future that resolves when this consumer subscription count
	/// reaches at least one. If the consumer is already subscribed, the future
	/// resolves immediately.
	pub async fn subscribed(&self) {
		let mut notified = self.notify.notified();
		loop {
			if self.is_subscribed() {
				return;
			}
			notified.await;
			notified = self.notify.notified();
		}
	}

	/// Returns a future that resolves when this consumer subscription count
	/// reaches zero. If the consumer is already unsubscribed, the future resolves
	/// immediately.
	pub async fn unsubscribed(&self) {
		let mut notified = self.notify.notified();
		loop {
			if !self.is_subscribed() {
				return;
			}
			notified.await;
			notified = self.notify.notified();
		}
	}

	/// Awaits until the number of connected producers changes.
	pub async fn producers_changed(&self) {
		let producers_count = self.producers_count();
		let mut notified = self.notify.notified();
		loop {
			if self.producers_count() != producers_count {
				break;
			}
			notified.await;
			notified = self.notify.notified();
		}
	}

	/// Awaits until this consumer has been terminated.
	pub async fn terminated(&self) {
		self.cancel.cancelled().await
	}
}
