use {
	crate::primitives::{IntoIterOrSingle, Tag},
	core::{
		pin::Pin,
		task::{Context, Poll},
	},
	std::collections::BTreeSet,
};

#[derive(Clone)]
pub struct When;

impl When {
	/// Returns a future that resolves when the producer has at least one
	/// subscriber.
	pub fn subscribed(&self) -> SubscriptionCondition {
		SubscriptionCondition {
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
pub struct SubscriptionCondition {
	min_subscribers: usize,
	required_tags: Option<BTreeSet<Tag>>,
}

impl SubscriptionCondition {
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

impl Future for SubscriptionCondition {
	type Output = ();

	fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
		Poll::Ready(())
	}
}
