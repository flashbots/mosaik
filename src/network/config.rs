use {
	super::{Network, NetworkId, error::Error},
	crate::{
		SecretKey,
		discovery::{self, Discovery},
		groups::{self, Groups},
		network::{LocalNode, ProtocolProvider},
		store::{self, Stores},
		streams::{self, Streams},
	},
	core::net::SocketAddr,
	derive_builder::Builder,
	iroh::{
		Endpoint,
		address_lookup::{MemoryLookup, mdns::MdnsAddressLookupBuilder},
		protocol::Router,
	},
	std::collections::BTreeSet,
};

/// Configuration options for the discovery subsystem.
#[derive(Debug, Clone, Builder)]
#[builder(
	pattern = "owned",
	name = NetworkBuilder,
	setter(prefix = with),
	build_fn(private, name = compile)
)]
pub struct NetworkConfig {
	/// Creates a new network builder with the given network ID.
	#[builder(setter(into))]
	pub network_id: NetworkId,

	/// Configures the use of relay servers for the network instance.
	/// This affects the ability to traverse NATs and firewalls.
	#[builder(default = "iroh::RelayMode::Default")]
	pub relay_mode: iroh::RelayMode,

	/// Enables mDNS discovery for local network peer discovery.
	#[builder(default = "false")]
	pub mdns_discovery: bool,

	/// Sets the local address for the network instance.
	/// This can be called multiple times to bind to multiple addresses.
	///
	/// By default, the network will bind to all interfaces.
	#[builder(setter(into), default = "BTreeSet::new()")]
	pub addresses: BTreeSet<SocketAddr>,

	/// Sets the secret key for the network instance.
	///
	/// This key is used to derive the node's identity, manually specifying it
	/// allows for deterministic identities across restarts.
	#[builder(setter(into), default = "SecretKey::generate(&mut rand::rng())")]
	pub secret_key: SecretKey,

	/// Configuration options for the discovery subsystem.
	///
	/// See [`discovery::Config`] for details.
	#[builder(default = "discovery::Config::builder()")]
	pub discovery: discovery::ConfigBuilder,

	/// Configuration options for the streams subsystem.
	///
	/// See [`streams::Config`] for details.
	#[builder(default = "streams::Config::builder()")]
	pub streams: streams::ConfigBuilder,

	/// Configuration options for the kv-stores subsystem.
	///
	/// See [`store::Config`] for details.
	#[builder(default = "store::Config::builder()")]
	pub stores: store::ConfigBuilder,

	/// Configuration options for the groups subsystem.
	///
	/// See [`groups::Config`] for details.
	#[builder(default = "groups::Config::builder()")]
	pub groups: groups::ConfigBuilder,
}

/// Public API
impl NetworkBuilder {
	/// Builds and returns a new `Network` instance.
	pub async fn build(self) -> Result<Network, Error> {
		let compiled = self.compile().map_err(|_| Error::MissingNetworkId)?;

		let endpoint = compiled.bind_endpoint().await?;
		let local = LocalNode::new(compiled.network_id, endpoint);

		// each components contributes its set of protocols to the router
		// when we have all protocols installed then we set the ready signal
		let mut protocols = Router::builder(local.endpoint().clone());

		// discovery
		let config = compiled.discovery.build()?;
		let discovery = Discovery::new(local.clone(), config);
		protocols = discovery.install(protocols);

		// streams
		let config = compiled.streams.build()?;
		let streams = Streams::new(local.clone(), &discovery, config);
		protocols = streams.install(protocols);

		// groups
		let config = compiled.groups.build()?;
		let groups = Groups::new(local.clone(), &discovery, config);
		protocols = groups.install(protocols);

		// stores
		let config = compiled.stores.build()?;
		let stores = Stores::new(local.clone(), &discovery, config);
		protocols = stores.install(protocols);

		// finalize router it will route incoming connections to protocols
		let router = protocols.spawn();

		// all protocols are installed, mark the local node as ready
		local.mark_ready();

		// Wait for the node to acquire a public address and be reachable by other
		// peers before returning the network instance.
		local.online().await;

		Ok(Network {
			local,
			discovery,
			streams,
			groups,
			stores,
			router,
		})
	}
}

/// Internal helpers
impl NetworkConfig {
	async fn bind_endpoint(&self) -> Result<Endpoint, Error> {
		let mut endpoint_builder = Endpoint::builder()
			.secret_key(self.secret_key.clone())
			.relay_mode(self.relay_mode.clone())
			.address_lookup(MemoryLookup::new());

		if self.mdns_discovery {
			endpoint_builder =
				endpoint_builder.address_lookup(MdnsAddressLookupBuilder::default());
		}

		for addr in &self.addresses {
			endpoint_builder = endpoint_builder.bind_addr(*addr)?;
		}

		Ok(endpoint_builder.bind().await?)
	}
}
