use {
	crate::prelude::Tag,
	core::{
		pin::Pin,
		sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
		task::{Context, Poll},
	},
	std::sync::Arc,
	tokio::sync::{Notify, futures::OwnedNotified},
	tokio_util::sync::CancellationToken,
};

/// This type provides status information and updates about a local stream
/// producer.
///
/// Notes:
/// - It can be used to wait until the producer is online, has subscribers, etc.
/// - It can be used to query the current snapshot of statistics about the
///   producer, such as the number of subscribers, amount of data sent, etc.
#[derive(Debug, Clone)]
pub struct Status(pub(super) Arc<Inner>);

#[derive(Debug)]
pub(super) struct Inner {
	/// The number of datums produced by this producer.
	/// This is independent of the number of consumers.
	pub(crate) items_sent: AtomicU64,

	/// The number of datums dropped by this producer because there were no
	/// consumers or none of the consumers criteria matched a produced datum.
	pub(crate) items_dropped: AtomicU64,

	/// The number of bytes produced by this producer.
	///
	/// This is independent of the number of consumers, this is just the raw
	/// size of the data produced by this stream as if it were sent to a single
	/// consumer.
	pub(crate) bytes_sent: AtomicU64,

	/// The cumulative number of bytes sent to all consumers.
	/// This counts duplicates sent to multiple consumers.
	pub(crate) cumulative_bytes_sent: AtomicU64,

	/// The current number of subscribers to this stream.
	pub(crate) subscribers_count: AtomicU32,

	/// When the producer is ready to accept consumers.
	pub(crate) online: AtomicBool,

	/// Receives notifications when important status changes occur.
	/// Currently it is notified when the producer goes online, or when the
	/// number of subscribers changes.
	pub(crate) notify: Arc<Notify>,

	/// Keeps track of this stream termination.
	pub(crate) cancel: CancellationToken,
}

impl Status {
	pub(crate) fn new() -> Self {
		Self(Arc::new(Inner {
			items_sent: AtomicU64::new(0),
			items_dropped: AtomicU64::new(0),
			bytes_sent: AtomicU64::new(0),
			cumulative_bytes_sent: AtomicU64::new(0),
			subscribers_count: AtomicU32::new(0),
			online: AtomicBool::new(false),
			notify: Arc::new(Notify::new()),
			cancel: CancellationToken::new(),
		}))
	}
}

impl Status {
	/// The current number of remote subscribers to this stream.
	pub fn subscribers_count(&self) -> u32 {
		self.0.subscribers_count.load(Ordering::Relaxed)
	}

	/// Whether the producer is currently online and is ready to accept
	/// consumers.
	pub fn is_online(&self) -> bool {
		self.0.online.load(Ordering::Relaxed)
	}

	/// Whether the producer currently has any subscribers.
	pub fn is_subscribed(&self) -> bool {
		self.subscribers_count() > 0
	}

	/// Whether the producer has been terminated.
	/// When terminated, the producer can no longer send data.
	pub fn is_terminated(&self) -> bool {
		self.0.cancel.is_cancelled()
	}

	/// The total number of items sent by this producer.
	pub fn items_sent(&self) -> u64 {
		self.0.items_sent.load(Ordering::Relaxed)
	}

	/// The total number of bytes created by this producer.
	pub fn bytes_sent(&self) -> u64 {
		self.0.bytes_sent.load(Ordering::Relaxed)
	}

	/// The cumulative number of bytes sent to all consumers.
	pub fn cumulative_bytes_sent(&self) -> u64 {
		self.0.cumulative_bytes_sent.load(Ordering::Relaxed)
	}

	/// Returns a future that resolves when the producer is online.
	/// If the producer is already online, the future resolves immediately.
	pub async fn online(&self) {
		let mut notified = self.0.notify.notified();
		if !self.is_online() {
			loop {
				notified.await;
				if self.is_online() {
					break;
				}

				notified = self.0.notify.notified();
			}
		}
	}

	/// Returns a future that allows awaiting certain subscription conditions.
	/// By default the returned future resolves when there is at least one
	/// subscriber.
	pub fn subscribed(&self) -> Subscribed {
		Subscribed {
			variant: WatchType::AtLeast(1),
			last_count: self.subscribers_count(),
			status: self.clone(),
			pending: Box::pin(self.0.notify.clone().notified_owned()),
		}
	}

	/// Returns a future that resolves when the producer transitions from having
	/// subscribers to having none. The future waits until it observes at least
	/// one subscriber, then resolves when all subscribers leave. Can be polled
	/// repeatedly to detect multiple transitions.
	pub fn unsubscribed(&self) -> Unsubscribed {
		Unsubscribed {
			status: self.clone(),
			seen_subscribed: self.subscribers_count() > 0,
			pending: Box::pin(self.0.notify.clone().notified_owned()),
		}
	}

	/// Returns a future that resolves when the producer has been terminated.
	/// If the producer is already terminated, the future resolves immediately.
	pub async fn terminated(&self) {
		self.0.cancel.cancelled().await;
	}
}

pub struct Subscribed {
	status: Status,
	variant: WatchType,
	last_count: u32,
	pending: Pin<Box<OwnedNotified>>,
}

pub struct Unsubscribed {
	status: Status,
	seen_subscribed: bool,
	pending: Pin<Box<OwnedNotified>>,
}

enum WatchType {
	Change,
	AtLeast(u32),
}

impl Subscribed {
	/// Returns a future that resolves when the producer has at least `count`
	/// subscribers. If the producer already has at least `count` subscribers,
	/// the future resolves immediately.
	#[must_use]
	pub fn by_at_least(self, count: u32) -> Self {
		Self {
			variant: WatchType::AtLeast(count),
			last_count: self.last_count,
			status: self.status,
			pending: self.pending,
		}
	}

	/// Returns a future that resolves when the producer has at least `count`
	/// subscribers with the given tag. If the producer already has at least
	/// `count` subscribers with the tag, the future resolves immediately.
	#[must_use]
	pub fn by_peers_with_tag(self, _tag: Tag) -> Self {
		todo!()
	}

	/// Returns a future that resolves when the number of subscribers changes.
	/// This future always waits for the next change, even if there are no
	/// subscribers currently.
	#[must_use]
	pub fn change(self) -> Self {
		Self {
			variant: WatchType::Change,
			last_count: self.last_count,
			status: self.status,
			pending: self.pending,
		}
	}
}

impl Future for Subscribed {
	type Output = ();

	fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
		let this = self.get_mut();

		loop {
			// Check if the condition is already met.
			let current_count = this.status.subscribers_count();
			match this.variant {
				WatchType::Change => {
					if current_count != this.last_count {
						return Poll::Ready(());
					}
				}
				WatchType::AtLeast(threshold) => {
					if current_count >= threshold {
						return Poll::Ready(());
					}
				}
			}

			// Poll the notification future.
			let pinned = Pin::new(&mut this.pending);
			match pinned.poll(cx) {
				Poll::Ready(()) => {
					// Notification received, but condition not met yet.
					// Create a new notified future for the next notification.
					this.pending =
						Box::pin(this.status.0.notify.clone().notified_owned());
				}
				Poll::Pending => {
					return Poll::Pending;
				}
			}
		}
	}
}

impl Future for Unsubscribed {
	type Output = ();

	fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
		let this = self.get_mut();

		loop {
			let count = this.status.subscribers_count();

			// If we have subscribers, mark that we've seen the subscribed state.
			if count > 0 {
				this.seen_subscribed = true;
			}

			// Only resolve if we've seen subscribers and now have none.
			if this.seen_subscribed && count == 0 {
				// Reset so next poll requires a new subscribed -> unsubscribed
				// transition.
				this.seen_subscribed = false;
				return Poll::Ready(());
			}

			// Poll the notification future.
			match this.pending.as_mut().poll(cx) {
				Poll::Ready(()) => {
					this.pending =
						Box::pin(this.status.0.notify.clone().notified_owned());
				}
				Poll::Pending => {
					return Poll::Pending;
				}
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use {
		super::Status,
		core::{sync::atomic::Ordering, time::Duration},
		tokio::time::timeout,
	};

	#[tokio::test(flavor = "multi_thread")]
	async fn online_waits_until_flag_is_set() {
		// Ensure `online()` stalls until we flip the internal flag and notify.
		let status = Status::new();
		let fut = status.online();
		tokio::pin!(fut);

		assert!(
			timeout(Duration::from_millis(20), fut.as_mut())
				.await
				.is_err()
		);

		status.0.online.store(true, Ordering::Relaxed);
		status.0.notify.notify_waiters();

		timeout(Duration::from_millis(50), fut).await.unwrap();
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn subscribed_at_least_waits_for_threshold() {
		// Wait until the subscriber count reaches the requested threshold.
		let status = Status::new();
		let fut = status.subscribed().by_at_least(2);
		tokio::pin!(fut);

		status.0.subscribers_count.store(1, Ordering::Relaxed);
		status.0.notify.notify_waiters();
		assert!(
			timeout(Duration::from_millis(20), fut.as_mut())
				.await
				.is_err()
		);

		status.0.subscribers_count.store(2, Ordering::Relaxed);
		status.0.notify.notify_waiters();
		timeout(Duration::from_millis(50), fut).await.unwrap();
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn unsubscribed_waits_until_no_subscribers() {
		// Ensure `unsubscribed()` only resolves after the count drops to zero.
		let status = Status::new();
		status.0.subscribers_count.store(1, Ordering::Relaxed);
		let fut = status.unsubscribed();
		tokio::pin!(fut);

		assert!(
			timeout(Duration::from_millis(20), fut.as_mut())
				.await
				.is_err()
		);

		status.0.subscribers_count.store(0, Ordering::Relaxed);
		status.0.notify.notify_waiters();
		timeout(Duration::from_millis(50), fut).await.unwrap();
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn subscribers_changed_fires_on_next_change() {
		// Detect the next change in subscriber count regardless of direction.
		let status = Status::new();
		let fut = status.subscribed().change();
		tokio::pin!(fut);

		assert!(
			timeout(Duration::from_millis(20), fut.as_mut())
				.await
				.is_err()
		);

		status.0.subscribers_count.store(1, Ordering::Relaxed);
		status.0.notify.notify_waiters();
		timeout(Duration::from_millis(50), fut).await.unwrap();
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn terminated_resolves_after_cancel() {
		// Verify `terminated()` stays pending until the cancellation token fires.
		let status = Status::new();
		let fut = status.terminated();
		tokio::pin!(fut);

		assert!(
			timeout(Duration::from_millis(20), fut.as_mut())
				.await
				.is_err()
		);

		status.0.cancel.cancel();
		timeout(Duration::from_millis(50), fut).await.unwrap();
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn subscribed_resolves_immediately_when_already_subscribed() {
		// If a subscriber already exists, `subscribed()` should short-circuit.
		let status = Status::new();
		status.0.subscribers_count.store(1, Ordering::Relaxed);

		timeout(Duration::from_millis(50), status.subscribed())
			.await
			.unwrap();
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn unsubscribed_waits_for_transition() {
		// Ensure `unsubscribed()` only resolves after transitioning from
		// subscribed to unsubscribed.
		let status = Status::new();

		// Start with no subscribers - should wait for subscribe then unsubscribe.
		let fut = status.unsubscribed();
		tokio::pin!(fut);

		assert!(
			timeout(Duration::from_millis(20), fut.as_mut())
				.await
				.is_err()
		);

		// Add a subscriber.
		status.0.subscribers_count.store(1, Ordering::Relaxed);
		status.0.notify.notify_waiters();

		// Still pending - we have subscribers.
		assert!(
			timeout(Duration::from_millis(20), fut.as_mut())
				.await
				.is_err()
		);

		// Remove all subscribers - now it should resolve.
		status.0.subscribers_count.store(0, Ordering::Relaxed);
		status.0.notify.notify_waiters();
		timeout(Duration::from_millis(50), fut).await.unwrap();
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn unsubscribed_resolves_when_starting_subscribed() {
		// If starting with subscribers, resolve when they leave.
		let status = Status::new();
		status.0.subscribers_count.store(1, Ordering::Relaxed);

		let fut = status.unsubscribed();
		tokio::pin!(fut);

		assert!(
			timeout(Duration::from_millis(20), fut.as_mut())
				.await
				.is_err()
		);

		status.0.subscribers_count.store(0, Ordering::Relaxed);
		status.0.notify.notify_waiters();
		timeout(Duration::from_millis(50), fut).await.unwrap();
	}
}
