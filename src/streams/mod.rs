//! Streams Consumers and Producers
//!
//! Notes:
//! - Streams are the primary dataflow primitive in Mosaik. They represent
//!   typed, asynchronous data channels that connect producers and consumers
//!   across a network.
//!
//! - Streams can be established between nodes that have discovered each other
//!   via the [`discovery`](crate::discovery) subsystem and have each other's
//!   entries in their local catalogs. Subscriptions from unknown consumers are
//!   rejected with a `PeerUnknown` error. That consumer should re-sync its
//!   local catalog with the producer and retry the subscription.

use {
	crate::{
		discovery::Discovery,
		network::{
			self,
			LocalNode,
			ProtocolProvider,
			link::{self, Protocol},
		},
		primitives::UniqueId,
	},
	accept::Acceptor,
	iroh::protocol::RouterBuilder,
	producer::Sinks,
	std::sync::Arc,
};

mod accept;
mod config;
mod criteria;
mod datum;

// Streams submodules
pub mod consumer;
pub mod producer;
pub mod status;

pub use {
	config::{Config, ConfigBuilder, ConfigBuilderError, backoff},
	consumer::Consumer,
	criteria::Criteria,
	datum::Datum,
	producer::Producer,
};

/// A unique identifier for a stream within the Mosaik network.
///
/// By default this id is derived from the stream datum type.
pub type StreamId = UniqueId;

/// The streams subsystem for a Mosaik network.
///
/// Streams are the primary dataflow primitive in Mosaik. They represent typed,
/// asynchronous data channels that connect producers and consumers across a
/// network.
pub struct Streams {
	/// Configuration for the streams subsystem.
	config: Arc<Config>,

	/// The local node instance associated with this streams subsystem.
	///
	/// This gives us access to the transport layer socket and identity.
	local: LocalNode,

	/// The discovery system used to announce newly created streams and find
	/// remote stream producers.
	discovery: Discovery,

	/// Map of active fanout sinks by stream id.
	sinks: Arc<Sinks>,
}

/// Public API
impl Streams {
	/// Creates a new [`Producer`] for the given data type `D` with default
	/// configuration.
	///
	/// For more advanced configuration, use [`Streams::producer`] to get a
	/// [`producer::Builder`] that can be customized.
	///
	/// If a producer for the given datum type already exists, it is returned
	/// instead of creating a new one and all the configuration set by the first
	/// producer is retained.
	pub fn produce<D: Datum>(&self) -> Producer<D> {
		match producer::Builder::new(self).build() {
			Ok(producer) => producer,
			Err(producer::BuilderError::AlreadyExists(existing)) => existing,
		}
	}

	/// Creates a new [`producer::Builder`] for the given data type `D` to
	/// assemble a more nuanced producer configuration.
	pub fn producer<D: Datum>(&self) -> producer::Builder<'_, D> {
		producer::Builder::new(self)
	}

	/// Creates a new [`Consumer`] for the given data type `D` with default
	/// configuration.
	///
	/// For more advanced configuration, use [`Streams::consumer`] to get a
	/// [`consumer::Builder`] that can be customized.
	pub fn consume<D: Datum>(&self) -> Consumer<D> {
		self.consumer().build()
	}

	/// Creates a new [`consumer::Builder`] for the given data type `D` to
	/// assemble a more nuanced consumer configuration.
	pub fn consumer<D: Datum>(&self) -> consumer::Builder<'_, D> {
		consumer::Builder::new(self)
	}
}

/// Internal construction API
impl Streams {
	/// Internally used by [`super::NetworkBuilder`] to create a new Streams
	/// subsystem instance as part of the overall [`super::Network`] instance.
	pub(crate) fn new(
		local: LocalNode,
		discovery: &Discovery,
		config: Config,
	) -> Self {
		Self {
			local: local.clone(),
			config: Arc::new(config),
			discovery: discovery.clone(),
			sinks: Arc::new(Sinks::new(local, discovery.clone())),
		}
	}
}

impl ProtocolProvider for Streams {
	fn install(&self, protocols: RouterBuilder) -> RouterBuilder {
		protocols.accept(Self::ALPN, Acceptor::new(self))
	}
}

impl link::Protocol for Streams {
	/// ALPN identifier for the streams protocol.
	const ALPN: &'static [u8] = b"/mosaik/streams/1.0";
}

network::error::make_close_reason!(
	/// The requested stream was not found on the producer node.
	struct StreamNotFound, 10_404);

network::error::make_close_reason!(
	/// The remote peer is not allowed to subscribe to the requested stream.
	struct NotAllowed, 10_403);

network::error::make_close_reason!(
	/// The producer has reached its maximum number of allowed subscribers
	/// and cannot accept any new consumer subscriptions.
	struct NoCapacity, 10_509);

network::error::make_close_reason!(
	/// The producer has disconnected the consumer due to slow consumption of
	/// data. The consumer should consider increasing its processing capacity
	/// or investigate network latency. See `ProducerConfig::disconnect_lagging` for
	/// more details.
	struct TooSlow, 10_413);
