use {
	crate::primitives::{IntoIterOrSingle, Tag},
	core::{
		pin::Pin,
		task::{Context, Poll},
	},
	std::{collections::BTreeSet, sync::Arc},
	tokio::sync::SetOnce,
};

/// Producer status monitoring
///
/// This struct provides access to futures that can be used to await changes
/// in the producer's status, such as when it gains subscribers or becomes
/// ready to interact with other peers.
#[derive(Clone)]
pub struct When {
	/// A one-time set handle that is completed when the producer worker loop is
	/// initialized and ready to interact with other peers.
	pub(super) ready: Arc<SetOnce<()>>,
}

// Internal API
impl When {
	pub(super) fn new(ready: Arc<SetOnce<()>>) -> Self {
		Self { ready }
	}
}

// Public API
impl When {
	/// Returns a future that resolves when the producer is ready to interact
	/// with other peers and has completed its initial setup.
	///
	/// Resolves immediately if the producer is already up and running.
	pub async fn online(&self) {
		self.ready.wait().await;
	}

	/// Returns a future that resolves when the producer has at least one
	/// subscriber.
	pub fn subscribed(&self) -> SubscriptionCondition {
		SubscriptionCondition {
			min_consumers: 1,
			required_tags: None,
		}
	}
}

/// A future that resolves when a producer's status meets a certain condition.
///
/// This future can be polled multiple times even after it has resolved once,
/// and it will resolve again when the awaited condition transitions again from
/// not met to met.
pub struct SubscriptionCondition {
	min_consumers: usize,
	required_tags: Option<BTreeSet<Tag>>,
}

impl SubscriptionCondition {
	/// Specifies that the future should resolve when there is at least the given
	/// number of consumers.
	pub fn by_at_least(mut self, min: usize) -> Self {
		self.min_consumers = min;
		self
	}

	/// Specifies that the future should resolve when it is subscribed by
	/// consumers that contain the given tags in their
	/// [`PeerEntry`](crate::discovery::PeerEntry).
	///
	/// When combined with `by_at_least`, the condition is met when there are at
	/// least that many subscribers with the given tags.
	pub fn with_tags<V>(mut self, tags: impl IntoIterOrSingle<Tag, V>) -> Self {
		self.required_tags = Some(tags.iterator().into_iter().collect());
		self
	}
}

impl Future for SubscriptionCondition {
	type Output = ();

	fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
		Poll::Ready(())
	}
}
