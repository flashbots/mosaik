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
	core::{any::Any, marker::PhantomData},
	tokio::sync::mpsc::UnboundedSender,
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

	/// If set to true, the producer will disconnect slow consumers that are
	/// unable to keep up with the data production rate. If the backlog of a
	/// consumer inflight datums grows beyond `buffer_size` it will be
	/// disconnected. Default to true.
	pub disconnect_lagging: bool,

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
	pub max_consumers: usize,

	/// Optional sink for unsent datum that were not delivered to any consumers
	/// because the datum did not meet any subscription criteria of any active
	/// consumers.
	///
	/// This is type-erased to allow config to be stored without
	/// knowing the datum type but is expected to be of type
	/// [`tokio::sync::mpsc::UnboundedSender<D>`].
	pub(crate) undelivered: Option<Box<dyn Any + Send + Sync>>,
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

	/// If set to true, the producer will disconnect slow consumers that are
	/// unable to keep up with the data production rate. If the backlog of a
	/// consumer inflight datums grows beyond `buffer_size` it will be
	/// disconnected.
	#[must_use]
	pub const fn disconnect_lagging(mut self, disconnect: bool) -> Self {
		self.config.disconnect_lagging = disconnect;
		self
	}

	/// Sets the buffer size for the producer's internal channel that holds datum
	/// before they are sent to connected consumers. If the buffer is full, calls
	/// to `send` on the producer will await until there is space available.
	#[must_use]
	pub const fn with_buffer_size(mut self, size: usize) -> Self {
		self.config.buffer_size = size;
		self
	}

	/// Sets the maximum number of subscribers allowed for this producer.
	/// Defaults to unlimited if not set.
	#[must_use]
	pub const fn with_max_consumers(mut self, max: usize) -> Self {
		self.config.max_consumers = max;
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

	/// Sets an optional sink for undelivered datum that were not delivered to
	/// any consumers because they did not meet any subscription criteria of
	/// active subscriptions or because there were no active subscribers.
	///
	/// Note that in default configuration, when there are not active
	/// subscribers, the producer is considered offline and will not accept any
	/// new datum to be sent. However, if the `online_when` condition is
	/// customized to allow publishing even when there are no subscribers, this
	/// sink can be used to capture datum that would otherwise be dropped.
	#[must_use]
	pub fn with_undelivered_sink(mut self, sink: UnboundedSender<D>) -> Self {
		self.config.undelivered = Some(Box::new(sink));
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
				disconnect_lagging: true,
				stream_id: D::derived_stream_id(),
				accept_if: Box::new(|_| true),
				online_when: Box::new(|c| c.minimum_of(1)),
				max_consumers: usize::MAX,
				network_id: *streams.local.network_id(),
				undelivered: None,
			},
			_marker: PhantomData,
		}
	}
}
