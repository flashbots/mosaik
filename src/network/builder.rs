use {
	super::{LocalNode, Network, NetworkError, NetworkId},
	crate::{
		Discovery,
		Groups,
		PeerId,
		ProtocolProvider,
		SecretKey,
		Streams,
		Tag,
	},
	core::net::SocketAddr,
	iroh::{Endpoint, protocol::Router},
	std::{collections::BTreeSet, io, net::ToSocketAddrs},
	tokio::sync::SetOnce,
};

/// A builder for creating network instances.
pub struct NetworkBuilder {
	network_id: NetworkId,
	secret_key: Option<SecretKey>,
	bootstrap: BTreeSet<PeerId>,
	addresses: BTreeSet<SocketAddr>,
	tags: BTreeSet<Tag>,
}

/// Public construction API
impl NetworkBuilder {
	/// Creates a new network builder with the given network ID.
	pub fn new(network_id: NetworkId) -> Self {
		Self {
			network_id,
			secret_key: None,
			bootstrap: BTreeSet::new(),
			addresses: BTreeSet::new(),
			tags: BTreeSet::new(),
		}
	}

	/// Sets the secret key for the network instance.
	///
	/// This key is used to derive the node's identity, manually specifying it
	/// allows for deterministic identities across restarts.
	#[must_use]
	pub fn with_secret_key(mut self, key: SecretKey) -> Self {
		self.secret_key = Some(key);
		self
	}

	/// Adds a bootstrap peer to the network instance.
	///
	/// On startup the network will attempt to connect to this peer to
	/// join the network and discover other peers.
	#[must_use]
	pub fn with_bootstrap_peer(mut self, peer: PeerId) -> Self {
		self.bootstrap.insert(peer);
		self
	}

	/// Adds bootstrap peers to the network instance.
	///
	/// On startup the network will attempt to connect to these peers to
	/// join the network and discover other peers.
	#[must_use]
	pub fn with_bootstrap_peers(
		mut self,
		peers: impl IntoIterator<Item = PeerId>,
	) -> Self {
		self.bootstrap.extend(peers);
		self
	}

	/// Adds a tag to the network instance.
	///
	/// Tags can be used to label and categorize the network instance for various
	/// purposes such as filtering, identification, or metadata association.
	#[must_use]
	pub fn with_tag(mut self, tag: Tag) -> Self {
		self.tags.insert(tag);
		self
	}

	/// Adds multiple tags to the network instance.
	///
	/// Tags can be used to label and categorize the network instance for various
	/// purposes such as filtering, identification, or metadata association.
	#[must_use]
	pub fn with_tags(mut self, tags: impl IntoIterator<Item = Tag>) -> Self {
		self.tags.extend(tags);
		self
	}

	/// Sets the local address for the network instance.
	/// This can be called multiple times to bind to multiple addresses.
	///
	/// By default, the network will bind to all interfaces.
	pub fn with_address(
		mut self,
		address: impl ToSocketAddrs,
	) -> Result<Self, io::Error> {
		self.addresses.extend(address.to_socket_addrs()?);
		Ok(self)
	}

	/// Builds and returns a new `Network` instance.
	pub async fn build(self) -> Result<Network, NetworkError> {
		let ready = SetOnce::new();
		let endpoint = self.bind_endpoint().await?;
		let local = LocalNode::new(self.network_id, endpoint, ready.clone());

		// each components contributes its set of protocols to the router
		// when we have all protocols installed then we set the ready signal
		let mut protocols = Router::builder(local.endpoint().clone());

		// discovery
		let discovery = Discovery::new(local.clone(), self.bootstrap);
		protocols = discovery.install(protocols);

		// streams
		let streams = Streams::new(local.clone(), &discovery);
		protocols = streams.install(protocols);

		// groups
		let groups = Groups::new(local.clone(), &discovery);
		protocols = groups.install(protocols);

		// finalize router it will route incoming connections to protocols
		let router = protocols.spawn();

		// this marks the local node as ready to accept connections
		// all protocols use this signal to know when they can start operating
		#[allow(clippy::missing_panics_doc)]
		ready.set(()).expect("this is a bug, already set;");

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
impl NetworkBuilder {
	async fn bind_endpoint(&self) -> Result<Endpoint, NetworkError> {
		let mut endpoint_builder = Endpoint::builder();
		if let Some(ref secret_key) = self.secret_key {
			endpoint_builder = endpoint_builder.secret_key(secret_key.clone());
		}
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
