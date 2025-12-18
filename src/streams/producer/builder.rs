use {
	super::{
		super::{Datum, Streams},
		Producer,
	},
	crate::{
		NetworkId,
		StreamId,
		discovery::PeerEntry,
		streams::status::ChannelConditions,
	},
	core::marker::PhantomData,
};

#[derive(Debug, thiserror::Error)]
pub enum Error<D: Datum> {
	/// A producer for the given stream id already exists.
	///
	/// This error is returned when attempting to create a new producer
	/// through the builder while one already exists for the same stream id.
	///
	/// If you need multiple producers for the same datum type, consider using
	/// the default `produce` method which allows multiple instances to
	/// share the same underlying stream.
	#[error("Producer for this stream id already exists")]
	AlreadyExists(Producer<D>),
}

/// Configuration options for a stream producer.
pub struct ProducerConfig {
	/// The stream id this producer is associated is producing for.
	/// There can only be one producer per stream id on one network node.
	pub stream_id: StreamId,

	/// The buffer size for the producer's internal channel that holds datum
	/// before they are sent to connected consumers. If the buffer is full, calls
	/// to `send` on the producer will await until there is space available.
	pub buffer_size: usize,

	/// Sets a predicate function that is used to determine whether to
	/// accept or reject incoming consumer connections.
	pub accept_if: Box<dyn Fn(&PeerEntry) -> bool + Send + Sync>,

	/// A function that specifies conditions under which a channel is
	/// considered online and can publish data to consumers. Here you can
	/// specify conditions such as minimum number of subscribers, required tags,
	/// or custom predicates, that must be met before publishing is allowed
	/// otherwise sending data through the producer will fail.
	///
	/// This follows the same API as the `producer.when().subscribed()` method.
	/// By default this is set to allow publishing if there is at least one
	/// subscriber.
	pub online_when:
		Box<dyn Fn(ChannelConditions) -> ChannelConditions + Send + Sync>,

	/// The network id this producer is associated with.
	pub network_id: NetworkId,

	/// Maximum number of subscribers allowed for this producer.
	///
	/// Defaults to unlimited if not set.
	pub max_subscribers: usize,
}

/// Configurable builder for assembling a new producer instances for a specific
/// datum type `D`.
pub struct Builder<'s, D: Datum> {
	config: ProducerConfig,
	streams: &'s Streams,
	_marker: PhantomData<D>,
}

/// Public API
impl<D: Datum> Builder<'_, D> {
	/// Sets a predicate function that is used to determine whether to
	/// accept or reject incoming consumer connections.
	#[must_use]
	pub fn accept_if<F>(mut self, pred: F) -> Self
	where
		F: Fn(&PeerEntry) -> bool + Send + Sync + 'static,
	{
		self.config.accept_if = Box::new(pred);
		self
	}

	/// A function that produces channel conditions under which a datum can be
	/// considered publishable to a consumer. Here you can specify conditions
	/// such as minimum number of subscribers, required tags, or custom
	/// predicates, that must be met before publishing is allowed otherwise
	/// sending data through the producer will fail.
	///
	/// This follows the same API as the `producer.when().subscribed()` method.
	/// By default this is set to allow publishing if there is at least one
	/// subscriber.
	#[must_use]
	pub fn online_when<F>(mut self, f: F) -> Self
	where
		F: Fn(ChannelConditions) -> ChannelConditions + Send + Sync + 'static,
	{
		self.config.online_when = Box::new(f);
		self
	}

	/// Sets the buffer size for the producer's internal channel that holds datum
	/// before they are sent to connected consumers. If the buffer is full, calls
	/// to `send` on the producer will await until there is space available.
	#[must_use]
	pub fn with_buffer_size(mut self, size: usize) -> Self {
		self.config.buffer_size = size;
		self
	}

	/// Sets the maximum number of subscribers allowed for this producer.
	/// Defaults to unlimited if not set.
	#[must_use]
	pub fn with_max_subscribers(mut self, max: usize) -> Self {
		self.config.max_subscribers = max;
		self
	}

	/// Sets the stream id this producer is associated is producing for.
	/// There can only be one producer per stream id on one network node.
	///
	/// If not set, defaults to the stream id of datum type `D`.
	#[must_use]
	pub fn with_stream_id(mut self, stream_id: impl Into<StreamId>) -> Self {
		self.config.stream_id = stream_id.into();
		self
	}

	/// Builds a new producer with the given configuration for this stream id.
	/// If there is already an existing producer for this stream id, an error
	/// is returned containing the existing producer created using the original
	/// configuration.
	pub fn build(self) -> Result<Producer<D>, Error<D>> {
		if let Some(existing) = self.streams.sinks.open(self.config.stream_id) {
			return Err(Error::AlreadyExists(existing.sender()));
		}

		self
			.streams
			.sinks
			.create::<D>(self.config)
			.map(|handle| handle.sender())
			.map_err(|existing| Error::AlreadyExists(existing.sender()))
	}
}

impl<'s, D: Datum> Builder<'s, D> {
	pub(in crate::streams) fn new(streams: &'s Streams) -> Self {
		Self {
			streams,
			config: ProducerConfig {
				buffer_size: 1024,
				stream_id: D::derived_stream_id(),
				accept_if: Box::new(|_| true),
				online_when: Box::new(|c| c.minimum_of(1)),
				max_subscribers: usize::MAX,
				network_id: *streams.local.network_id(),
			},
			_marker: PhantomData,
		}
	}
}
