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
	std::net::SocketAddrV4,
	tokio_util::sync::CancellationToken,
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

impl Network {
	async fn run(
		network_id: NetworkId,
		endpoint: Endpoint,
		static_provider: StaticProvider,
		bootstrap_peers: Vec<iroh::EndpointId>,
		cancel: CancellationToken,
	) -> Result<Self, Error> {
		let me = Local::new(endpoint, network_id, static_provider, cancel);

		// wait for the local peer to be online
		me.online().await;

		let mut streams = Streams::new(me.clone());
		let mut discovery = Discovery::new(&me, bootstrap_peers);

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
}

/// Public network management API
impl Network {
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

pub struct NetworkBuilder {
	network_id: NetworkId,
	endpoint_builder: iroh::endpoint::Builder,
	static_provider: StaticProvider,
	bootstrap_peers: Vec<iroh::EndpointAddr>,
	cancellation_token: Option<CancellationToken>,
}

impl NetworkBuilder {
	/// Creates a new `NetworkBuilder` for the given `NetworkId`.
	///
	/// If no secret key is provided, a new random key will be generated.
	///
	/// If no port is provided, a random available port will be used.
	pub fn new(network_id: NetworkId) -> Self {
		let static_provider = StaticProvider::new();
		Self {
			network_id,
			endpoint_builder: Endpoint::builder().discovery(static_provider.clone()),
			static_provider,
			bootstrap_peers: Vec::new(),
			cancellation_token: None,
		}
	}

	#[must_use]
	pub fn with_bootstrap_peer(mut self, peer: iroh::EndpointAddr) -> Self {
		self.bootstrap_peers.push(peer);
		self
	}

	#[must_use]
	pub fn with_bootstrap_peers(
		mut self,
		peers: Vec<iroh::EndpointAddr>,
	) -> Self {
		self.bootstrap_peers.extend(peers);
		self
	}

	#[must_use]
	pub fn with_secret_key(mut self, secret_key: iroh::SecretKey) -> Self {
		self.endpoint_builder = self.endpoint_builder.secret_key(secret_key);
		self
	}

	#[must_use]
	pub fn with_port(mut self, port: u16) -> Self {
		let socket_addr = SocketAddrV4::new(std::net::Ipv4Addr::UNSPECIFIED, port);
		self.endpoint_builder = self.endpoint_builder.bind_addr_v4(socket_addr);
		self
	}

	#[must_use]
	pub fn with_cancellation_token(mut self, token: CancellationToken) -> Self {
		self.cancellation_token = Some(token);
		self
	}

	/// Builds and runs a new `Network` instance with the given `NetworkId`.
	pub async fn build_and_run(self) -> Result<Network, Error> {
		let Self {
			network_id,
			endpoint_builder,
			static_provider,
			bootstrap_peers,
			cancellation_token,
		} = self;

		let bootstrap_peer_ids = bootstrap_peers
			.iter()
			.map(|addr| addr.id)
			.collect::<Vec<_>>();
		for bootstrap_peer in bootstrap_peers {
			static_provider.add_endpoint_info(bootstrap_peer);
		}

		let endpoint = endpoint_builder
			.discovery(static_provider.clone())
			.bind()
			.await?;

		Network::run(
			network_id,
			endpoint,
			static_provider,
			bootstrap_peer_ids,
			cancellation_token.unwrap_or_else(CancellationToken::new),
		)
		.await
	}
}
