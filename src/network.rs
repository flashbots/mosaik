use {
	crate::{
		datum::{Criteria, Datum},
		discovery::Discovery,
		id::NetworkId,
		local::Local,
		prelude::DiscoveryError,
		streams::{Consumer, Producer, Streams},
	},
	core::{
		pin::Pin,
		task::{Context, Poll},
	},
	iroh::{
		Endpoint,
		discovery::static_provider::StaticProvider,
		protocol::Router,
	},
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("Bind error: {0}")]
	Bind(#[from] iroh::endpoint::BindError),

	#[error("Discovery error: {0}")]
	Discovery(#[from] DiscoveryError),
}

/// Represents a connection to the Mosaik network.
pub struct Network {
	me: Local,
	discovery: Discovery,
	_protocols: Router,
}

/// Public network management API
impl Network {
	/// Creates a new Network instance with the given `NetworkId`.
	///
	/// This will generate a random peer identity and bind to a random port.
	pub async fn new(network_id: NetworkId) -> Result<Self, Error> {
		Self::with_endpoint(network_id, Endpoint::bind().await?).await
	}

	/// Creates a new Network instance with the given `NetworkId` and Endpoint.
	pub async fn with_endpoint(
		network_id: NetworkId,
		endpoint: Endpoint,
	) -> Result<Self, Error> {
		let static_provider = StaticProvider::new();
		endpoint.discovery().add(static_provider.clone());
		Self::with_endpoint_and_provider(network_id, endpoint, static_provider)
			.await
	}

	/// Creates a new Network instance with the given `NetworkId` and Endpoint.
	pub async fn with_endpoint_and_provider(
		network_id: NetworkId,
		endpoint: Endpoint,
		static_provider: StaticProvider,
	) -> Result<Self, Error> {
		let me = Local::new(endpoint, network_id, static_provider);

		// wait for the local peer to be online
		me.online().await;

		let mut streams = Streams::new(me.clone());
		let mut discovery = Discovery::new(&me);

		let builder = Router::builder(me.endpoint().clone());
		let builder = streams.attach(builder);
		let builder = discovery.attach(builder);
		let protocols = builder.spawn();

		Ok(Self {
			me,
			discovery,
			_protocols: protocols,
		})
	}

	/// Returns the endpoint of the local node.
	pub const fn local(&self) -> &Local {
		&self.me
	}

	/// Returns the discovery module of this network.
	pub const fn discovery(&self) -> &Discovery {
		&self.discovery
	}

	/// Returns the `NetworkId` of this network.
	pub fn network_id(&self) -> &NetworkId {
		self.me.network_id()
	}
}

/// Public pubsub API
impl Network {
	pub fn produce<D: Datum>(&self) -> Producer<D> {
		self.me.create_sink::<D>().producer::<D>()
	}

	pub fn consume<D: Datum>(&self) -> Consumer<D> {
		self.consume_with(Criteria::default())
	}

	pub fn consume_with<D: Datum>(&self, criteria: Criteria) -> Consumer<D> {
		Consumer::<D>::new(self, criteria)
	}
}

impl Future for Network {
	type Output = ();

	fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
		Poll::Pending
	}
}
