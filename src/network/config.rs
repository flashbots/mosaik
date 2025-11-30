use {
	super::{Network, NetworkId, error::Error},
	crate::{
		Discovery,
		Groups,
		LocalNode,
		ProtocolProvider,
		SecretKey,
		Streams,
		discovery,
		streams,
	},
	core::net::SocketAddr,
	derive_builder::Builder,
	iroh::{Endpoint, protocol::Router},
	serde::{Deserialize, Serialize},
	std::collections::BTreeSet,
};

/// Configuration options for the discovery subsystem.
#[derive(Debug, Clone, Builder, Serialize, Deserialize)]
#[builder(
	pattern = "owned",
	name = "NetworkBuilder",
	setter(prefix = "with"),
	build_fn(private, name = "compile")
)]
pub struct NetworkConfig {
	/// Creates a new network builder with the given network ID.
	#[builder(setter(into))]
	network_id: NetworkId,

	/// Sets the local address for the network instance.
	/// This can be called multiple times to bind to multiple addresses.
	///
	/// By default, the network will bind to all interfaces.
	#[builder(setter(into), default = "BTreeSet::new()")]
	addresses: BTreeSet<SocketAddr>,

	/// Sets the secret key for the network instance.
	///
	/// This key is used to derive the node's identity, manually specifying it
	/// allows for deterministic identities across restarts.
	#[builder(setter(into), default = "SecretKey::generate(&mut rand::rng())")]
	secret_key: SecretKey,

	/// Configuration options for the discovery subsystem.
	///
	/// See [`discovery::Config`] for details.
	#[builder(default = "discovery::Config::builder().build().unwrap()")]
	discovery: discovery::Config,

	/// Configuration options for the streams subsystem.
	///
	/// See [`streams::Config`] for details.
	#[builder(default = "streams::Config::builder().build().unwrap()")]
	streams: streams::Config,
}

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
		let config = compiled.discovery;
		let discovery = Discovery::new(local.clone(), config);
		protocols = discovery.install(protocols);

		// streams
		let config = compiled.streams;
		let streams = Streams::new(local.clone(), discovery.clone(), config);
		protocols = streams.install(protocols);

		// groups
		let groups = Groups::new(local.clone(), discovery.clone());
		protocols = groups.install(protocols);

		// finalize router it will route incoming connections to protocols
		let router = protocols.spawn();

		// all protocols are installed, mark the local node as ready
		local.mark_ready();

		Ok(Network {
			local,
			discovery,
			streams,
			groups,
			router,
		})
	}
}

/// Internal helpers
impl NetworkConfig {
	async fn bind_endpoint(&self) -> Result<Endpoint, Error> {
		let mut endpoint_builder =
			Endpoint::builder().secret_key(self.secret_key.clone());

		for addr in &self.addresses {
			match addr {
				SocketAddr::V4(addr) => {
					endpoint_builder = endpoint_builder.bind_addr_v4(*addr);
				}
				SocketAddr::V6(addr) => {
					endpoint_builder = endpoint_builder.bind_addr_v6(*addr);
				}
			}
		}
		Ok(endpoint_builder.bind().await?)
	}
}
