//! Streams Consumers and Producers

use {
	crate::{
		discovery::Discovery,
		network::{LocalNode, ProtocolProvider},
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

pub use {
	config::{Config, ConfigBuilder, ConfigBuilderError},
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
	sinks: Sinks,
}

/// Public API
impl Streams {
	/// Creates a new [`Producer`] for the given data type `D` with default
	/// configuration.
	///
	/// For more advanced configuration, use [`Streams::producer`] to get a
	/// [`producer::Builder`] that can be customized.
	pub fn produce<D: Datum>(&self) -> Producer<D> {
		self.producer().build()
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
	/// ALPN identifier for the streams protocol.
	const ALPN: &'static [u8] = b"/mosaik/streams/1.0";

	/// Internally used by [`super::NetworkBuilder`] to create a new Streams
	/// subsystem instance as part of the overall [`super::Network`] instance.
	pub(crate) fn new(
		local: LocalNode,
		discovery: Discovery,
		config: Config,
	) -> Self {
		let config = Arc::new(config);
		Self {
			local,
			config: Arc::clone(&config),
			discovery: discovery.clone(),
			sinks: Sinks::new(discovery, config),
		}
	}
}

impl ProtocolProvider for Streams {
	fn install(&self, protocols: RouterBuilder) -> RouterBuilder {
		protocols.accept(Self::ALPN, Acceptor::new(self))
	}
}
