use {
	super::ActiveChannelsMap,
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
	tokio::sync::{SetOnce, watch},
	tokio_util::sync::ReusableBoxFuture,
};

/// Awaits changes to the channel's status.
///
/// This struct provides access to a future that can be used to await when
/// the channel becomes online and ready to interact with other peers or meets
/// other subscription conditions.
#[derive(Clone)]
pub struct When {
	/// A one-time set handle that is completed when the channel is
	/// online and ready to interact with other peers.
	pub(crate) ready: Arc<SetOnce<()>>,

	/// Observer for the most recent version of the active subscriptions info.
	pub(crate) active: watch::Receiver<ActiveChannelsMap>,
}

impl When {
	/// Initialized by consumers and producers for each new stream subscription.
	pub(crate) fn new(
		active: watch::Receiver<ActiveChannelsMap>,
		ready: Arc<SetOnce<()>>,
	) -> Self {
		Self { ready, active }
	}
}

// Public API
impl When {
	/// Returns a future that resolves when the consumer or producer is ready to
	/// interact with other peers and has completed its initial setup.
	///
	/// Resolves immediately if the consumer or producer is already up and
	/// running.
	pub async fn online(&self) {
		self.ready.wait().await;
	}

	/// Returns a future that resolves when the consumer or producer is subscribed
	/// to at least one peer. This can be customized and combined with other
	/// conditions using the methods on the returned [`SubscriptionCondition`].
	pub fn subscribed(&self) -> SubscriptionCondition {
		let mut active = self.active.clone();
		active.mark_changed();

		SubscriptionCondition {
			active: active.clone(),
			min_peers: 1,
			was_met: false,
			is_inverse: false,
			predicates: Vec::new(),
			changed_fut: ReusableBoxFuture::new(Box::pin(async move {
				let _ = active.changed().await;
			})),
		}
	}

	/// Returns a future that resolves when the consumer or producer does not have
	/// any subscriptions. This can be customized and combined with
	/// other conditions using the methods on the returned
	/// [`SubscriptionCondition`].
	///
	/// This is equivalent to calling `subscribed().not()`.
	pub fn unsubscribed(&self) -> SubscriptionCondition {
		self.subscribed().not()
	}
}

/// A future that resolves when a producer or consumer subscription status meets
/// a certain condition.
///
/// This future can be polled multiple times even after it has resolved once,
/// and it will resolve again when the awaited condition transitions again from
/// not met to met.
///
/// In its initial state when instantiated and the condition is met immediately,
/// the future will resolve on the next poll, then reset to awaiting state until
/// the condition transitions from not met to met.
pub struct SubscriptionCondition {
	active: watch::Receiver<ActiveChannelsMap>,
	min_peers: usize,
	predicates: Vec<Arc<PeerPredicate>>,
	was_met: bool,
	is_inverse: bool,
	changed_fut: ReusableBoxFuture<'static, ()>,
}

// Public API
impl SubscriptionCondition {
	/// Specifies that the future should resolve when there is at least the given
	/// number of peers.
	pub fn minimum_of(mut self, min: usize) -> Self {
		self.min_peers = min;
		self
	}

	/// Specifies that the future should resolve when it is subscribed to
	/// producers that contain the given tags in their
	/// [`PeerEntry`](crate::discovery::PeerEntry).
	///
	/// When combined with `minimum_of`, the condition is met when there are at
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

	/// Checks the minimum number of connected peers that meet our predicates.
	pub fn is_condition_met(&self) -> bool {
		let active = self.active.borrow();
		let matching_peers = active
			.values()
			.filter(|handle| {
				handle.is_connected()
					&& self.predicates.iter().all(|pred| pred(&handle.peer))
			})
			.count();

		(matching_peers >= self.min_peers) != self.is_inverse
	}

	/// Inverts the condition, so that it resolves when the condition is not met.
	pub fn not(self) -> Self {
		let mut cloned = self.clone();
		cloned.is_inverse = !cloned.is_inverse;
		cloned
	}
}

impl Clone for SubscriptionCondition {
	fn clone(&self) -> Self {
		let mut active = self.active.clone();
		active.mark_changed();
		Self {
			active: active.clone(),
			min_peers: self.min_peers,
			predicates: self.predicates.clone(),
			was_met: false,
			is_inverse: self.is_inverse,
			changed_fut: ReusableBoxFuture::new(Box::pin(async move {
				let _ = active.changed().await;
			})),
		}
	}
}

impl fmt::Debug for SubscriptionCondition {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("SubscriptionCondition")
			.field("min_peers", &self.min_peers)
			.field("predicates", &self.predicates.len())
			.field("is_condition_met", &self.is_condition_met())
			.finish_non_exhaustive()
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

impl PartialEq<bool> for SubscriptionCondition {
	fn eq(&self, other: &bool) -> bool {
		self.is_condition_met() == *other
	}
}

impl PartialEq<SubscriptionCondition> for bool {
	fn eq(&self, other: &SubscriptionCondition) -> bool {
		*self == other.is_condition_met()
	}
}

type PeerPredicate = dyn Fn(&PeerEntry) -> bool + Send + Sync;
