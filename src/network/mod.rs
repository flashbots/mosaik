//! Network management module

use {
	crate::{
		discovery::Discovery,
		groups::Groups,
		primitives::UniqueId,
		streams::Streams,
	},
	iroh::protocol::{Router, RouterBuilder},
};

mod config;
mod error;
mod local;

pub(crate) mod link;

pub use {
	config::{NetworkBuilder, NetworkConfig},
	error::Error as NetworkError,
	local::LocalNode,
};

/// The entrypoint to the Mosaic network SDK.
///
/// Notes:
///
/// - This type represents a Mosaik network connection and provides methods to
///   access different facilities such as peer discovery and stream management.
///
/// - In most cases one network instance will represent one running
///   process/node.
///
/// - Each network instance has a globally unique identity that is the public
///   key of the secret key used to construct it. If no secret key is provided a
///   new random key will be generated. This is an important property because
///   node identities are globally unique across all Mosaik networks and nodes
///   can be addressed by their public keys alone. The discovery mechanism is
///   responsible for resolving the corresponding network addresses for a given
///   public key. Knowing the physical transport addresses of a peer is not
///   sufficient to identify a node and connect to it.
///
/// - By default a random local port will be chosen for the network instance.
///   This can be overridden by providing a specific port in the builder.
///
/// - Mosaik networks are identified by a [`NetworkId`] which is a unique
///   identifier (a sha3-256 hash of the network name). Nodes can only connect
///   to other nodes that are part of the same network (i.e. have the same
///   [`NetworkId`]).
pub struct Network {
	/// The local node instance.
	/// Maintains the transport layer endpoint and addresses.
	local: LocalNode,

	/// The discovery system.
	discovery: Discovery,

	/// Streams system
	streams: Streams,

	/// Groups system
	groups: Groups,

	/// The protocol router.
	/// Responsible for routing incoming connections to the appropriate protocol
	/// handlers. See [`ProtocolProvider`] and [`NetworkBuilder::build`] for more
	/// details.
	///
	/// this is not dead code, it keeps the router event loop alive and its
	/// lifetime bound to the network instance.
	#[allow(dead_code)]
	router: Router,
}

/// This type uniquely identifies a network by its name.
///
/// On the protocol level this is represented by a gossip `TopicId` that is a
/// 32-byte sha3-256 hash of the network name bytes.
pub type NetworkId = UniqueId;

/// This type uniquely identifies a peer globally across all Mosaik networks.
/// It is the public key derived from the node's secret key.
pub type PeerId = iroh::EndpointId;

/// Public construction API
impl Network {
	/// Creates a new network builder with a given network id.
	pub fn builder(network_id: NetworkId) -> NetworkBuilder {
		NetworkBuilder::default().with_network_id(network_id)
	}

	/// Creates and returns a new [`Network`] instance with the given network ID
	/// and default settings.
	pub async fn new(network_id: NetworkId) -> Result<Self, NetworkError> {
		NetworkBuilder::default()
			.with_network_id(network_id)
			.build()
			.await
	}
}

/// Public API
impl Network {
	/// Returns the network identifier of the network that this instance is
	/// connected to.
	pub fn network_id(&self) -> &NetworkId {
		self.local.network_id()
	}

	/// Returns a reference to the local node instance.
	pub const fn local(&self) -> &LocalNode {
		&self.local
	}

	/// Returns a future that resolves when the local node is considered to be
	/// online and ready to interact with other peers.
	pub async fn online(&self) {
		self.local.online().await;
	}

	/// Returns a reference to the discovery system.
	pub const fn discovery(&self) -> &Discovery {
		&self.discovery
	}

	/// Returns a reference to the streams system.
	pub const fn streams(&self) -> &Streams {
		&self.streams
	}

	/// Returns a reference to the groups system.
	pub const fn groups(&self) -> &Groups {
		&self.groups
	}
}

/// Internal trait for network components that need to install new ALPNs during
/// network initialization.
pub(crate) trait ProtocolProvider {
	/// Installs the protocol's ALPN handler into the given router builder.
	fn install(&self, protocols: RouterBuilder) -> RouterBuilder;
}
