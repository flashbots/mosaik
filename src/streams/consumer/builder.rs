use {
	crate::{
		Criteria,
		StreamId,
		discovery::PeerInfo,
		primitives::BackoffFactory,
		streams::{
			Consumer,
			Datum,
			Streams,
			consumer::worker,
			status::ChannelConditions,
		},
		tickets::TicketValidator,
	},
	backoff::backoff::Backoff,
	core::marker::PhantomData,
	std::sync::Arc,
};

pub struct ConsumerConfig {
	/// The stream id this consumer is subscribing to.
	pub stream_id: StreamId,

	/// Specifies the criteria for the range of data this consumer is interested
	/// in.
	pub criteria: Criteria,

	/// Holds the predicate that decides if a producer is eligible and should be
	/// contacted for establishing a subscription.
	pub require: Box<dyn Fn(&PeerInfo) -> bool + Send + Sync>,

	/// The backoff policy for retrying stream subscription connections on
	/// recoverable failures.
	pub backoff: BackoffFactory,

	/// A function that specifies conditions under which the consumer is
	/// considered online. Here you can specify conditions such as minimum
	/// number of connected producers, required tags, or custom predicates.
	///
	/// This follows the same API as the `consumer.when().subscribed()`
	/// method. By default this is set to always consider the consumer
	/// online as soon as it starts (minimum of 0 producers).
	pub online_when:
		Box<dyn Fn(ChannelConditions) -> ChannelConditions + Send + Sync>,

	/// Ticket validators for authenticating producer peers.
	///
	/// When non-empty, producers must present valid tickets that pass all
	/// configured validators before the consumer will subscribe to them.
	/// Tickets that carry an expiration are tracked and the consumer
	/// automatically disconnects from producers when their earliest ticket
	/// expires.
	pub ticket_validators: Vec<Arc<dyn TicketValidator>>,
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
	pub const fn with_criteria(mut self, criteria: Criteria) -> Self {
		self.config.criteria = criteria;
		self
	}

	/// Adds a ticket validator for authenticating producer peers.
	///
	/// Each discovered producer must present valid tickets that pass all
	/// configured validators before the consumer will subscribe. If a
	/// ticket carries an expiration, the consumer will automatically
	/// disconnect from the producer when the earliest ticket expires.
	/// Can be called multiple times to require multiple types of tickets.
	#[must_use]
	pub fn require_ticket(mut self, validator: impl TicketValidator) -> Self {
		self.config.ticket_validators.push(Arc::new(validator));
		self
	}

	/// Adds a peer eligibility requirement for producers this consumer will
	/// subscribe to.
	///
	/// The predicate receives a [`PeerInfo`] combining the producer's
	/// self-reported [`PeerEntry`](crate::discovery::PeerEntry) with
	/// locally-observed metrics like RTT. Since `PeerInfo` implements
	/// `Deref<Target = PeerEntry>`, existing predicates that call methods
	/// like `peer.tags()` continue to work unchanged.
	///
	/// When called multiple times, all predicates must pass — AND
	/// composition — so a producer is eligible only if every requirement
	/// is satisfied. Predicates are also re-evaluated dynamically when a
	/// peer's catalog entry changes (e.g. tags are added or removed),
	/// causing the consumer to connect or disconnect accordingly.
	///
	/// The default requirement considers all producers eligible.
	#[must_use]
	pub fn require<F>(mut self, pred: F) -> Self
	where
		F: Fn(&PeerInfo) -> bool + Send + Sync + 'static,
	{
		let prev = self.config.require;
		self.config.require = Box::new(move |peer| prev(peer) && pred(peer));
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

	/// Sets the stream id this consumer is subscribing to.
	///
	/// If not set, defaults to the stream id of datum type `D`.
	#[must_use]
	pub fn with_stream_id(mut self, stream_id: impl Into<StreamId>) -> Self {
		self.config.stream_id = stream_id.into();
		self
	}

	/// A function that produces channel conditions under which the consumer
	/// is considered online. Here you can specify conditions such as minimum
	/// number of connected producers, required tags, or custom predicates.
	///
	/// This follows the same API as the `consumer.when().subscribed()`
	/// method. By default the consumer is always considered online as soon
	/// as it starts.
	#[must_use]
	pub fn online_when<F>(mut self, f: F) -> Self
	where
		F: Fn(ChannelConditions) -> ChannelConditions + Send + Sync + 'static,
	{
		self.config.online_when = Box::new(f);
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
				stream_id: D::derived_stream_id(),
				criteria: Criteria::default(),
				require: Box::new(|_| true),
				backoff: streams.config.backoff.clone(),
				online_when: Box::new(|c| c.minimum_of(0)),
				ticket_validators: Vec::new(),
			},
			streams,
			_marker: PhantomData,
		}
	}
}
