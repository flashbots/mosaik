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
		primitives::{Datum, Digest},
	},
	accept::Acceptor,
	iroh::protocol::RouterBuilder,
	producer::Sinks,
	std::sync::Arc,
};

mod accept;
mod config;
mod criteria;
// Streams submodules
pub mod consumer;
pub mod producer;
pub mod status;

pub use {
	config::{Config, ConfigBuilder, ConfigBuilderError, backoff},
	consumer::Consumer,
	criteria::Criteria,
	producer::Producer,
};

/// Trait for stream definitions that provide a producer constructor.
///
/// Implemented automatically by the [`stream!`] macro. The generated
/// implementation bakes in any `require`, `online_when`, or other
/// producer configuration specified in the macro invocation.
pub trait StreamProducer {
	type Producer;

	fn producer(network: &crate::Network) -> Self::Producer;

	/// Creates a producer and waits for it to come online.
	fn online_producer(
		network: &crate::Network,
	) -> impl Future<Output = Self::Producer> + Send + Sync + 'static;
}

/// Trait for stream definitions that provide a consumer constructor.
///
/// Implemented automatically by the [`stream!`] macro. The generated
/// implementation bakes in any `require` or other consumer
/// configuration specified in the macro invocation.
pub trait StreamConsumer {
	type Consumer;

	fn consumer(network: &crate::Network) -> Self::Consumer;

	/// Creates a consumer and waits for it to come online.
	fn online_consumer(
		network: &crate::Network,
	) -> impl Future<Output = Self::Consumer> + Send + Sync + 'static;
}

/// Convenience type alias for the producer type of a stream definition.
pub type ProducerOf<S> = <S as StreamProducer>::Producer;

/// Convenience type alias for the consumer type of a stream definition.
pub type ConsumerOf<S> = <S as StreamConsumer>::Consumer;

/// A compile-time definition of a stream producer that can be used to create
/// a pre-configured [`producer::Builder`] for a given datum type with an
/// optional predefined [`StreamId`].
pub struct ProducerDef<T: Datum> {
	pub stream_id: Option<StreamId>,
	_marker: core::marker::PhantomData<fn(&T)>,
}

impl<T: Datum> ProducerDef<T> {
	pub const fn new(stream_id: Option<StreamId>) -> Self {
		Self {
			stream_id,
			_marker: core::marker::PhantomData,
		}
	}

	/// Returns a [`producer::Builder`] with the stream id pre-configured.
	#[inline]
	pub fn open<'s>(
		&self,
		network: &'s crate::Network,
	) -> producer::Builder<'s, T> {
		let mut builder = network.streams().producer::<T>();
		if let Some(id) = self.stream_id {
			builder = builder.with_stream_id(id);
		}
		builder
	}
}

/// A compile-time definition of a stream consumer that can be used to create
/// a pre-configured [`consumer::Builder`] for a given datum type with an
/// optional predefined [`StreamId`].
pub struct ConsumerDef<T: Datum> {
	pub stream_id: Option<StreamId>,
	_marker: core::marker::PhantomData<fn(&T)>,
}

impl<T: Datum> ConsumerDef<T> {
	pub const fn new(stream_id: Option<StreamId>) -> Self {
		Self {
			stream_id,
			_marker: core::marker::PhantomData,
		}
	}

	/// Returns a [`consumer::Builder`] with the stream id pre-configured.
	#[inline]
	pub fn open<'s>(
		&self,
		network: &'s crate::Network,
	) -> consumer::Builder<'s, T> {
		let mut builder = network.streams().consumer::<T>();
		if let Some(id) = self.stream_id {
			builder = builder.with_stream_id(id);
		}
		builder
	}
}

/// Declares a named stream definition with an optional compile-time
/// `StreamId` and baked-in configuration.
///
/// # Syntax
///
/// ```ignore
/// // Type-derived StreamId (most common):
/// stream!(pub MyStream = String);
///
/// // Explicit StreamId:
/// stream!(pub MyStream = String, "my.stream.id");
///
/// // With configuration:
/// stream!(pub MyStream = PriceUpdate, "oracle.price",
///     require: |peer| peer.tags().contains(&tag!("trusted")),
///     online_when: |c| c.minimum_of(2),
/// );
///
/// // Side-prefixed config (producer/consumer specific):
/// stream!(pub MyStream = PriceUpdate,
///     producer online_when: |c| c.minimum_of(2),
///     consumer online_when: |c| c.minimum_of(1),
/// );
///
/// // Producer only:
/// stream!(pub producer MyStream = String, "my.stream.id",
///     require: |peer| true,
/// );
///
/// // Consumer only:
/// stream!(pub consumer MyStream = String,
///     require: |peer| true,
/// );
/// ```
///
/// # Configuration keys
///
/// Producer-side (inferred):
/// - `require` — predicate for accepting consumer connections (AND-composed when repeated)
/// - `max_consumers` — maximum number of consumers
/// - `buffer_size` — internal channel buffer size
/// - `disconnect_lagging` — disconnect slow consumers
///
/// Consumer-side (inferred):
/// - `require` — predicate for selecting eligible producers (AND-composed when repeated)
/// - `criteria` — data range criteria
/// - `backoff` — retry backoff policy
///
/// Both sides (prefix with `producer`/`consumer` to target one):
/// - `online_when` — conditions under which the stream is online
///
/// # Usage
///
/// ```ignore
/// use mosaik::streams::{StreamProducer, StreamConsumer};
///
/// stream!(pub PriceFeed = PriceUpdate, "oracle.price",
///     require: |peer| true,
///     producer online_when: |c| c.minimum_of(2),
/// );
///
/// let producer = PriceFeed::producer(&network);
/// let consumer = PriceFeed::consumer(&network);
/// ```
#[macro_export]
macro_rules! stream {
	(#[$($meta:tt)*] $($rest:tt)*) => {
		$crate::stream! { @attrs [#[$($meta)*]] $($rest)* }
	};
	(@attrs [$($attrs:tt)*] #[$($meta:tt)*] $($rest:tt)*) => {
		$crate::stream! { @attrs [$($attrs)* #[$($meta)*]] $($rest)* }
	};
	(@attrs [$($attrs:tt)*] $($rest:tt)*) => {
		$crate::__stream_impl! { @$crate; $($attrs)* $($rest)* }
	};
	($($tt:tt)*) => {
		$crate::__stream_impl! { @$crate; $($tt)* }
	};
}

/// A unique identifier for a stream within the Mosaik network.
///
/// By default this id is derived from the stream datum type.
pub type StreamId = Digest;

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

	/// Creates a new [`producer::Builder`] for the given stream definition to
	/// assemble a more nuanced producer configuration.
	#[allow(clippy::needless_pass_by_value)]
	pub fn producer_of<D: Datum>(
		&self,
		def: StreamDef<D>,
	) -> producer::Builder<'_, D> {
		let mut builder = self.producer::<D>();
		if let Some(stream_id) = def.stream_id {
			builder = builder.with_stream_id(stream_id);
		}
		builder
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

	/// Creates a new [`consumer::Builder`] for the given stream definition to
	/// assemble a more nuanced consumer configuration.
	#[allow(clippy::needless_pass_by_value)]
	pub fn consumer_of<D: Datum>(
		&self,
		def: StreamDef<D>,
	) -> consumer::Builder<'_, D> {
		let mut builder = self.consumer::<D>();
		if let Some(stream_id) = def.stream_id {
			builder = builder.with_stream_id(stream_id);
		}
		builder
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

/// A stream definition that can be used to create a producer and consumer for a
/// given datum type.
///
/// Usually used by libraries that want to expose a well-known stream interface.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct StreamDef<T: Datum> {
	pub stream_id: Option<StreamId>,
	_marker: core::marker::PhantomData<fn(&T)>,
}

impl<T: Datum> Clone for StreamDef<T> {
	fn clone(&self) -> Self {
		*self
	}
}
impl<T: Datum> Copy for StreamDef<T> {}

impl<T: Datum> Default for StreamDef<T> {
	fn default() -> Self {
		Self::new()
	}
}

impl<T: Datum> StreamDef<T> {
	pub const fn new() -> Self {
		Self {
			stream_id: None,
			_marker: core::marker::PhantomData,
		}
	}

	/// Overrides the default type-derived stream id for this stream definition
	/// with a custom stream id.
	#[must_use]
	pub const fn with_stream_id(stream_id: StreamId) -> Self {
		Self {
			stream_id: Some(stream_id),
			_marker: core::marker::PhantomData,
		}
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
