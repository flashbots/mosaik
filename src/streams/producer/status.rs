use {
	crate::primitives::{IntoIterOrSingle, Tag},
	core::{
		pin::Pin,
		task::{Context, Poll},
	},
	std::collections::BTreeSet,
};

pub struct Status;

impl Status {
	/// Returns a future that resolves when the producer has at least one
	/// subscriber.
	pub fn subscribed(&self) -> SubscriptionConditionFuture {
		SubscriptionConditionFuture {
			min_subscribers: 1,
			required_tags: None,
		}
	}
}

/// A future that resolves when a producer's status meets a certain condition.
///
/// This future can be polled multiple times even after it has resolved once,
/// and it will resolve again when the awaited condition transitions again from
/// not met to met.
pub struct SubscriptionConditionFuture {
	min_subscribers: usize,
	required_tags: Option<BTreeSet<Tag>>,
}

impl SubscriptionConditionFuture {
	/// Specifies that the future should resolve when there is at least the given
	/// number of subscribers.
	pub fn by_at_least(mut self, min: usize) -> Self {
		self.min_subscribers = min;
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

impl Future for SubscriptionConditionFuture {
	type Output = ();

	fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
		Poll::Ready(())
	}
}
