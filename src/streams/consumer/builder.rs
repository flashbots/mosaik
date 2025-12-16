use {
	super::{
		super::{Streams, config::BackoffFactory},
		Consumer,
		Datum,
		worker,
	},
	crate::{Criteria, discovery::PeerEntry},
	backoff::backoff::Backoff,
	core::marker::PhantomData,
	std::sync::Arc,
};

pub(in crate::streams) struct ConsumerConfig {
	/// Specifies the criteria for the range of data this consumer is interested
	/// in.
	pub criteria: Criteria,

	/// Holds the predicate that decides if a producer is eligible and should be
	/// contacted for establishing a subscription.
	pub subscribe_if: Box<dyn Fn(&PeerEntry) -> bool + Send + Sync>,

	/// The backoff policy for retrying stream subscription connections on
	/// recoverable failures.
	pub backoff: BackoffFactory,
}

/// Configurable builder for assembling a new consumer instance for a specific
/// datum type `D`.
pub struct Builder<'s, D: Datum> {
	config: ConsumerConfig,
	streams: &'s Streams,
	_marker: PhantomData<D>,
}

impl<D: Datum> Builder<'_, D> {
	/// Sets the criteria for the range of data this consumer is interested in.
	#[must_use]
	pub fn with_criteria(mut self, criteria: Criteria) -> Self {
		self.config.criteria = criteria;
		self
	}

	/// Sets the predicate that adds additional user-defined eligibility criteria
	/// for producers that are going to be contacted for establishing
	/// subscriptions.
	#[must_use]
	pub fn subscribe_if<F>(mut self, pred: F) -> Self
	where
		F: Fn(&PeerEntry) -> bool + Send + Sync + 'static,
	{
		self.config.subscribe_if = Box::new(pred);
		self
	}

	/// The backoff policy for retrying stream subscription connections on
	/// recoverable failures for this consumer. If not set, the default backoff
	/// policy from the streams config is used.
	#[must_use]
	pub fn with_backoff<B: Backoff + Clone + Send + Sync + 'static>(
		mut self,
		backoff: B,
	) -> Self {
		self.config.backoff = Arc::new(move || Box::new(backoff.clone()));
		self
	}
}

impl<D: Datum> Builder<'_, D> {
	/// Builds the consumer instance and returns the receiver handle.
	pub fn build(self) -> Consumer<D> {
		worker::ConsumerWorker::<D>::spawn(self.config, self.streams)
	}
}

impl<'s, D: Datum> Builder<'s, D> {
	pub(in crate::streams) fn new(streams: &'s Streams) -> Self {
		Self {
			config: ConsumerConfig {
				criteria: Criteria::default(),
				subscribe_if: Box::new(|_| true),
				backoff: streams.config.backoff.clone(),
			},
			streams,
			_marker: PhantomData,
		}
	}
}
