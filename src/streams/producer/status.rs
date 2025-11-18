use {
	core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
	tokio::sync::Notify,
	tokio_util::sync::CancellationToken,
};

/// This type provides status information and updates about a local stream
/// producer.
///
/// Notes:
/// - It can be used to wait until the producer is online, has subscribers, etc.
/// - It can be used to query the current snapshot of statistics about the
///   producer, such as the number of subscribers, amount of data sent, etc.
#[derive(Debug)]
pub struct Status {
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
	pub(crate) notify: Notify,

	/// Keeps track of this stream termination.
	pub(crate) cancel: CancellationToken,
}

impl Status {
	pub(crate) fn new() -> Self {
		Self {
			items_sent: AtomicU64::new(0),
			items_dropped: AtomicU64::new(0),
			bytes_sent: AtomicU64::new(0),
			cumulative_bytes_sent: AtomicU64::new(0),
			subscribers_count: AtomicU32::new(0),
			online: AtomicBool::new(false),
			notify: Notify::new(),
			cancel: CancellationToken::new(),
		}
	}
}

impl Status {
	/// The current number of remote subscribers to this stream.
	pub fn subscribers_count(&self) -> u32 {
		self.subscribers_count.load(Ordering::Relaxed)
	}

	/// Whether the producer is currently online and is ready to accept
	/// consumers.
	pub fn is_online(&self) -> bool {
		self.online.load(Ordering::Relaxed)
	}

	/// Whether the producer currently has any subscribers.
	pub fn is_subscribed(&self) -> bool {
		self.subscribers_count() > 0
	}

	/// Whether the producer has been terminated.
	/// When terminated, the producer can no longer send data.
	pub fn is_terminated(&self) -> bool {
		self.cancel.is_cancelled()
	}

	/// The total number of items sent by this producer.
	pub fn items_sent(&self) -> u64 {
		self.items_sent.load(Ordering::Relaxed)
	}

	/// The total number of bytes created by this producer.
	pub fn bytes_sent(&self) -> u64 {
		self.bytes_sent.load(Ordering::Relaxed)
	}

	/// The cumulative number of bytes sent to all consumers.
	pub fn cumulative_bytes_sent(&self) -> u64 {
		self.cumulative_bytes_sent.load(Ordering::Relaxed)
	}

	/// Returns a future that resolves when the producer is online.
	/// If the producer is already online, the future resolves immediately.
	pub async fn online(&self) {
		let mut notified = self.notify.notified();
		if !self.is_online() {
			loop {
				notified.await;
				if self.is_online() {
					break;
				}

				notified = self.notify.notified();
			}
		}
	}

	/// Returns a future that resolves when the producer has no subscribers.
	/// If the producer has no subscribers, the future resolves immediately.
	/// If the producer currently has subscribers, the future waits until there
	/// are none.
	pub async fn unsubscribed(&self) {
		let mut notified = self.notify.notified();
		if self.is_subscribed() {
			loop {
				notified.await;
				if !self.is_subscribed() {
					break;
				}

				notified = self.notify.notified();
			}
		}
	}

	/// Returns a future that resolves when the producer has at least one
	/// subscriber. If the producer already has subscribers, the future
	/// resolves immediately.
	pub async fn subscribed(&self) {
		self.subscribed_at_least(1).await;
	}

	/// Returns a future that resolves when the producer has at least `count`
	/// subscribers. If the producer already has at least `count` subscribers,
	/// the future resolves immediately.
	pub async fn subscribed_at_least(&self, count: u32) {
		let mut notified = self.notify.notified();
		if self.subscribers_count() < count {
			loop {
				notified.await;
				if self.subscribers_count() >= count {
					break;
				}

				notified = self.notify.notified();
			}
		}
	}

	/// Returns a future that resolves when the number of subscribers changes.
	/// This future always waits for the next change, even if there are no
	/// subscribers currently.
	pub async fn subscribers_changed(&self) {
		let subs_count = self.subscribers_count();
		loop {
			self.notify.notified().await;
			if self.subscribers_count() != subs_count {
				break;
			}
		}
	}

	/// Returns a future that resolves when the producer has been terminated.
	/// If the producer is already terminated, the future resolves immediately.
	pub async fn terminated(&self) {
		self.cancel.cancelled().await;
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

		assert!(timeout(Duration::from_millis(20), fut.as_mut()).await.is_err());

		status.online.store(true, Ordering::Relaxed);
		status.notify.notify_waiters();

		timeout(Duration::from_millis(50), fut).await.unwrap();
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn subscribed_at_least_waits_for_threshold() {
		// Wait until the subscriber count reaches the requested threshold.
		let status = Status::new();
		let fut = status.subscribed_at_least(2);
		tokio::pin!(fut);

		status.subscribers_count.store(1, Ordering::Relaxed);
		status.notify.notify_waiters();
		assert!(timeout(Duration::from_millis(20), fut.as_mut()).await.is_err());

		status.subscribers_count.store(2, Ordering::Relaxed);
		status.notify.notify_waiters();
		timeout(Duration::from_millis(50), fut).await.unwrap();
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn unsubscribed_waits_until_no_subscribers() {
		// Ensure `unsubscribed()` only resolves after the count drops to zero.
		let status = Status::new();
		status.subscribers_count.store(1, Ordering::Relaxed);
		let fut = status.unsubscribed();
		tokio::pin!(fut);

		assert!(timeout(Duration::from_millis(20), fut.as_mut()).await.is_err());

		status.subscribers_count.store(0, Ordering::Relaxed);
		status.notify.notify_waiters();
		timeout(Duration::from_millis(50), fut).await.unwrap();
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn subscribers_changed_fires_on_next_change() {
		// Detect the next change in subscriber count regardless of direction.
		let status = Status::new();
		let fut = status.subscribers_changed();
		tokio::pin!(fut);

		assert!(timeout(Duration::from_millis(20), fut.as_mut()).await.is_err());

		status.subscribers_count.store(1, Ordering::Relaxed);
		status.notify.notify_waiters();
		timeout(Duration::from_millis(50), fut).await.unwrap();
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn terminated_resolves_after_cancel() {
		// Verify `terminated()` stays pending until the cancellation token fires.
		let status = Status::new();
		let fut = status.terminated();
		tokio::pin!(fut);

		assert!(timeout(Duration::from_millis(20), fut.as_mut()).await.is_err());

		status.cancel.cancel();
		timeout(Duration::from_millis(50), fut).await.unwrap();
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn subscribed_resolves_immediately_when_already_subscribed() {
		// If a subscriber already exists, `subscribed()` should short-circuit.
		let status = Status::new();
		status.subscribers_count.store(1, Ordering::Relaxed);

		timeout(Duration::from_millis(50), status.subscribed())
			.await
			.unwrap();
	}
}
