use {
	crate::{Discovery, LocalNode, ProtocolProvider, UniqueId},
	dashmap::DashMap,
	iroh::protocol::RouterBuilder,
	listen::Listener,
	producer::FanoutSink,
	std::sync::Arc,
};

mod config;
mod datum;
mod listen;

// Streams submodules
pub mod consumer;
pub mod producer;

pub use {
	config::{Config, ConfigBuilder},
	consumer::Consumer,
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
	config: Config,

	/// The local node instance associated with this streams subsystem.
	///
	/// This gives us access to the transport layer socket and identity.
	local: LocalNode,

	/// The discovery system used to announce newly created streams and find
	/// remote stream producers.
	discovery: Discovery,

	/// Map of active fanout sinks by stream id.
	sinks: Arc<DashMap<StreamId, FanoutSink>>,
}

/// Public API
impl Streams {
	/// Creates a new [`Producer`] for the given data type `D` with default
	/// configuration.
	///
	/// For more advanced configuration, use [`Streams::producer`] to get a
	/// [`ProducerBuilder`] that can be customized.
	pub async fn produce<D: Datum>(&self) -> Producer<D> {
		self.producer().build().await
	}

	/// Creates a new [`ProducerBuilder`] for the given data type `D` to
	/// assemble a more nuanced producer configuration.
	pub fn producer<D: Datum>(&self) -> producer::Builder<'_, D> {
		producer::Builder::new(self)
	}

	/// Creates a new [`Consumer`] for the given data type `D` with default
	/// configuration.
	///
	/// For more advanced configuration, use [`Streams::consumer`] to get a
	/// [`ConsumerBuilder`] that can be customized.
	pub fn consume<D: Datum>(&self) -> Consumer<D> {
		self.consumer().build()
	}

	/// Creates a new [`ConsumerBuilder`] for the given data type `D` to
	/// assemble a more nuanced consumer configuration.
	pub fn consumer<D: Datum>(&self) -> consumer::Builder<'_, D> {
		consumer::Builder::new(self)
	}
}

/// Internal construction API
impl Streams {
	const ALPN: &'static [u8] = b"/mosaik/streams/1";

	pub(crate) fn new(
		local: LocalNode,
		discovery: Discovery,
		config: Config,
	) -> Self {
		Self {
			config,
			local,
			discovery,
			sinks: Arc::new(DashMap::new()),
		}
	}
}

impl ProtocolProvider for Streams {
	fn install(&self, protocols: RouterBuilder) -> RouterBuilder {
		protocols.accept(Self::ALPN, Listener::new(self))
	}
}
