use {
	super::worker::ActiveReceivers,
	crate::{
		discovery::PeerEntry,
		primitives::{IntoIterOrSingle, Tag},
	},
	core::{
		fmt,
		pin::Pin,
		task::{Context, Poll},
	},
	futures::FutureExt,
	std::{collections::BTreeSet, sync::Arc},
	tokio::sync::watch,
	tokio_util::sync::ReusableBoxFuture,
};

/// Consumer status condition monitoring
///
/// This struct provides access to futures that can be used to await changes
/// in the consumer's status, such as when it becomes subscribed to producers or
/// disconnects from them.
pub struct When(watch::Receiver<ActiveReceivers>);

impl When {
	/// Creates a new `When` instance from the given active receivers observer.
	pub(super) fn new(active: watch::Receiver<ActiveReceivers>) -> Self {
		Self(active)
	}
}

impl When {
	/// Returns a future that resolves when the consumer is subscribed to at least
	/// one producer. This can be customized and combined with other conditions
	/// using the methods on the returned [`SubscriptionCondition`].
	pub fn subscribed(&self) -> SubscriptionCondition {
		let mut receiver = self.0.clone();
		SubscriptionCondition {
			active: self.0.clone(),
			min_producers: 1,
			was_met: false,
			is_inverse: false,
			predicates: Vec::new(),
			changed_fut: ReusableBoxFuture::new(Box::pin(async move {
				let _ = receiver.changed().await;
			})),
		}
	}

	/// Returns a future that resolves when the consumer does not have any
	/// subscriptions to producers. This can be customized and combined with other
	/// conditions using the methods on the returned [`SubscriptionCondition`].
	///
	/// This is equivalent to calling `subscribed().not()`.
	pub fn unsubscribed(&self) -> SubscriptionCondition {
		self.subscribed().not()
	}
}
/// A future that resolves when a consumer's status meets a certain condition.
///
/// This future can be polled multiple times even after it has resolved once,
/// and it will resolve again when the awaited condition transitions again from
/// not met to met.
///
/// In its initial state when instantiated and the condition is met immediately,
/// the future will resolve on the next poll, then reset to awaiting state until
/// the condition transitions from not met to met.
pub struct SubscriptionCondition {
	active: watch::Receiver<ActiveReceivers>,
	min_producers: usize,
	predicates: Vec<Arc<PeerPredicate>>,
	was_met: bool,
	is_inverse: bool,
	changed_fut: ReusableBoxFuture<'static, ()>,
}

impl Clone for SubscriptionCondition {
	fn clone(&self) -> Self {
		let mut receiver = self.active.clone();
		Self {
			active: receiver.clone(),
			min_producers: self.min_producers,
			predicates: self.predicates.clone(),
			was_met: false,
			is_inverse: self.is_inverse,
			changed_fut: ReusableBoxFuture::new(Box::pin(async move {
				let _ = receiver.changed().await;
			})),
		}
	}
}

impl fmt::Debug for SubscriptionCondition {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("SubscriptionCondition")
			.field("min_producers", &self.min_producers)
			.field("predicates", &self.predicates.len())
			.field("is_condition_met", &self.is_condition_met())
			.finish_non_exhaustive()
	}
}

// Public API
impl SubscriptionCondition {
	/// Specifies that the future should resolve when there is at least the given
	/// number of producers.
	#[expect(clippy::wrong_self_convention)]
	pub fn to_at_least(mut self, min: usize) -> Self {
		self.min_producers = min;
		self
	}

	/// Specifies that the future should resolve when it is subscribed to
	/// producers that contain the given tags in their
	/// [`PeerEntry`](crate::discovery::PeerEntry).
	///
	/// When combined with `to_at_least`, the condition is met when there are at
	/// least that many producers with the given tags.
	pub fn with_tags<V>(self, tags: impl IntoIterOrSingle<Tag, V>) -> Self {
		let tags: BTreeSet<Tag> = tags.iterator().into_iter().collect();
		self.with_predicate(move |peer: &PeerEntry| tags.is_subset(peer.tags()))
	}

	/// Specifies a custom predicate that must be met by producers for the
	/// condition to be considered met.
	pub fn with_predicate<F>(mut self, predicate: F) -> Self
	where
		F: Fn(&PeerEntry) -> bool + Send + Sync + 'static,
	{
		self.predicates.push(Arc::new(predicate));
		self
	}

	/// Checks the number of connected producers that meet our predicates.
	pub fn is_condition_met(&self) -> bool {
		let active = self.active.borrow();
		let matching_producers = active
			.values()
			.filter(|handle| {
				handle.is_connected()
					&& self.predicates.iter().all(|pred| pred(handle.peer()))
			})
			.count();

		(matching_producers >= self.min_producers) != self.is_inverse
	}

	/// Inverts the condition, so that it resolves when the condition is not met.
	pub fn not(self) -> Self {
		let mut cloned = self.clone();
		cloned.is_inverse = !cloned.is_inverse;
		cloned
	}
}

impl Future for SubscriptionCondition {
	type Output = ();

	fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
		let this = self.get_mut();

		loop {
			// First check if the condition is currently met
			let condition_met = this.is_condition_met();

			if condition_met && !this.was_met {
				// Transition from not met -> met (or initially met)
				this.was_met = true;
				return Poll::Ready(());
			}

			// Update state tracking
			this.was_met = condition_met;

			// Poll the stored changed future to wait for updates
			match this.changed_fut.poll_unpin(cx) {
				Poll::Ready(()) => {
					// The watch was updated, set up a new changed future and loop
					let mut receiver = this.active.clone();
					this.changed_fut.set(Box::pin(async move {
						let _ = receiver.changed().await;
					}));
				}
				Poll::Pending => return Poll::Pending,
			}
		}
	}
}

type PeerPredicate = dyn Fn(&PeerEntry) -> bool + Send + Sync;
