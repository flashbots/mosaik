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
	tokio::sync::watch,
	tokio_util::sync::ReusableBoxFuture,
};

/// Awaits changes to the channel's status.
///
/// This struct provides access to a future that can be used to await when
/// the channel becomes online and ready to interact with other peers or meets
/// other subscription conditions.
pub struct When {
	/// Observer for the ready status of the consumer or producer.
	///
	/// When the value is set to false the consumer or producer is not ready
	/// to send or receive data.
	pub(crate) online: watch::Receiver<bool>,

	/// Observer for the most recent version of the active subscriptions info.
	pub(crate) active: watch::Receiver<ActiveChannelsMap>,
}

impl Clone for When {
	fn clone(&self) -> Self {
		Self::new(self.active.clone(), self.online.clone())
	}
}

impl When {
	/// Initialized by consumers and producers for each new stream subscription.
	pub(crate) fn new(
		mut active: watch::Receiver<ActiveChannelsMap>,
		mut online: watch::Receiver<bool>,
	) -> Self {
		active.mark_changed();
		online.mark_changed();
		Self { online, active }
	}
}

// Public API
impl When {
	/// Returns a future that resolves when the consumer or producer is ready to
	/// send or receive data from other peers.
	///
	/// Resolves immediately if the consumer or producer is already up and
	/// running and all publishing criteria are met.
	///
	/// By default for producers, this means at least one subscriber is connected.
	pub fn online(&self) -> impl Future<Output = ()> + Send + Sync + 'static {
		let mut online = self.online.clone();

		async move {
			if online.wait_for(|v| *v).await.is_err() {
				// if the watch channel is closed, consider the consumer/producer
				// offline and never resolve this future
				core::future::pending::<()>().await;
			}
		}
	}

	/// Returns whether the consumer or producer is currently ready to send or
	/// receive data from other peers.
	pub fn is_online(&self) -> bool {
		*self.online.borrow()
	}

	/// Returns a future that resolves when the consumer or producer is not ready
	/// to send or receive data from other peers.
	///
	/// Resolves immediately if the consumer or producer is already offline.
	pub fn offline(&self) -> impl Future<Output = ()> + Send + Sync + 'static {
		let mut online = self.online.clone();

		async move {
			// if the watch channel is closed, consider the consumer/producer offline
			// and always resolve this future
			let _ = online.wait_for(|v| !*v).await;
		}
	}

	/// Returns a future that resolves when the consumer or producer is subscribed
	/// to at least one peer. This can be customized and combined with other
	/// conditions using the methods on the returned [`SubscriptionCondition`].
	pub fn subscribed(&self) -> ChannelConditions {
		let mut active = self.active.clone();
		active.mark_changed();

		ChannelConditions {
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
	pub fn unsubscribed(&self) -> ChannelConditions {
		self.subscribed().unmet()
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
pub struct ChannelConditions {
	active: watch::Receiver<ActiveChannelsMap>,
	min_peers: usize,
	predicates: Vec<Arc<PeerPredicate>>,
	was_met: bool,
	is_inverse: bool,
	changed_fut: ReusableBoxFuture<'static, ()>,
}

// Public API
impl ChannelConditions {
	/// Specifies that the future should resolve when there is at least the given
	/// number of peers.
	#[must_use]
	pub const fn minimum_of(mut self, min: usize) -> Self {
		self.min_peers = min;
		self
	}

	/// Specifies that the future should resolve when it is subscribed to
	/// producers that contain the given tags in their
	/// [`PeerEntry`](crate::discovery::PeerEntry).
	///
	/// When combined with `minimum_of`, the condition is met when there are at
	/// least that many producers with the given tags.
	#[must_use]
	pub fn with_tags<V>(self, tags: impl IntoIterOrSingle<Tag, V>) -> Self {
		let tags: BTreeSet<Tag> = tags.iterator().into_iter().collect();
		self.with_predicate(move |peer: &PeerEntry| tags.is_subset(peer.tags()))
	}

	/// Specifies a custom predicate that must be met by producers for the
	/// condition to be considered met.
	#[must_use]
	pub fn with_predicate<F>(mut self, predicate: F) -> Self
	where
		F: Fn(&PeerEntry) -> bool + Send + Sync + 'static,
	{
		self.predicates.push(Arc::new(predicate));
		self
	}

	/// Checks the minimum number of connected peers that meet our predicates.
	pub fn is_condition_met(&self) -> bool {
		let matching_peers = self
			.active
			.borrow()
			.values()
			.filter(|handle| {
				handle.is_connected()
					&& self.predicates.iter().all(|pred| pred(&handle.peer))
			})
			.count();

		(matching_peers >= self.min_peers) != self.is_inverse
	}

	/// Inverts the condition, so that the future resolves when the condition is
	/// not met.
	#[must_use]
	pub const fn unmet(self) -> Self {
		let mut cloned = self;
		cloned.is_inverse = true;
		cloned
	}
}

impl Clone for ChannelConditions {
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

impl fmt::Debug for ChannelConditions {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("SubscriptionCondition")
			.field("min_peers", &self.min_peers)
			.field("predicates", &self.predicates.len())
			.field("is_condition_met", &self.is_condition_met())
			.finish_non_exhaustive()
	}
}

impl Future for ChannelConditions {
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

impl PartialEq<bool> for ChannelConditions {
	fn eq(&self, other: &bool) -> bool {
		self.is_condition_met() == *other
	}
}

impl PartialEq<ChannelConditions> for bool {
	fn eq(&self, other: &ChannelConditions) -> bool {
		*self == other.is_condition_met()
	}
}

type PeerPredicate = dyn Fn(&PeerEntry) -> bool + Send + Sync;
