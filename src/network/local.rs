use {
	crate::{SecretKey, network::NetworkId},
	iroh::{Endpoint, EndpointAddr},
	std::{fmt, sync::Arc},
	tokio::sync::SetOnce,
	tokio_util::sync::CancellationToken,
};

/// This type represents the local node in the Mosaik network.
///
/// Notes:
/// - This type is cheap to clone; all clones refer to the same underlying
///   instance that is created and owned by the [`super::Network`] type.
///
/// - This type provides access to the transport layer of the local node for
///   establishing new connections to remote peers.
///
/// - This type maintains the acceptor for incoming connections from remote
///   peers.
///
/// - This type is responsible for maintaining the up to list of transport-level
///   addresses and their changes over time.
pub struct LocalNode(Arc<Inner>);

/// Public API
impl LocalNode {
	/// Returns the network identifier of the local node.
	pub fn network_id(&self) -> &NetworkId {
		&self.0.network_id
	}

	/// Returns the transport layer endpoint of the local node.
	pub fn endpoint(&self) -> &Endpoint {
		&self.0.endpoint
	}

	/// Returns the current transport layer address of the local node.
	pub fn addr(&self) -> EndpointAddr {
		self.endpoint().addr()
	}

	/// Returns the globally unique identifier of the local node.
	/// This is also the public key derived from the node's secret key.
	pub fn id(&self) -> iroh::EndpointId {
		self.0.endpoint.id()
	}

	/// Returns the secret key of the local node.
	///
	/// This key is the private key corresponding to the node's public identity
	/// and is used for signing and authenticating the node in the network. It is
	/// also used to sign [`PeerEntry`](crate::discovery::PeerEntry)s advertised
	/// by the node.
	pub fn secret_key(&self) -> &SecretKey {
		self.0.endpoint.secret_key()
	}

	/// Returns a future that resolves when the local node is considered to be
	/// online and ready to interact with other peers.
	pub async fn online(&self) {
		self.0.ready_signal.wait().await;
		self.0.endpoint.online().await;
	}
}

/// Internal API
impl LocalNode {
	/// Creates a new local node instance with the given network ID and endpoint.
	///
	/// This is used by the [`super::NetworkBuilder`] to construct the local node
	/// as part of building the overall network instance.
	pub(crate) fn new(network_id: NetworkId, endpoint: Endpoint) -> Self {
		Self(Arc::new(Inner {
			network_id,
			endpoint,
			ready_signal: SetOnce::new(),
			termination: CancellationToken::new(),
		}))
	}

	/// Marks the local node as ready to accept connections connections from
	/// remote peers. All protocols use this signal to know when they can start
	/// operating and have been installed in the protocol router.
	pub(crate) fn mark_ready(&self) {
		let _ = self.0.ready_signal.set(());
	}

	/// Returns a reference to the cancellation token that is triggered when
	/// the running network instance is being shut down or has unrecoverable
	/// failure.
	///
	/// Anything with access to this token can use it to shut down the network
	/// instance gracefully and all its associated protocols.
	pub(crate) fn termination(&self) -> &CancellationToken {
		&self.0.termination
	}
}

impl Clone for LocalNode {
	fn clone(&self) -> Self {
		Self(Arc::clone(&self.0))
	}
}

impl fmt::Debug for LocalNode {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("LocalNode")
			.field("network_id", &self.0.network_id)
			.field("endpoint", &self.0.endpoint)
			.finish()
	}
}

impl fmt::Display for LocalNode {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "LocalNode({})", self.0.endpoint.id())
	}
}

/// Inner state of the local node carried across all clones of [`LocalNode`].
struct Inner {
	/// The network identifier of the local node.
	network_id: NetworkId,

	/// The transport layer endpoint of the local node.
	endpoint: Endpoint,

	/// A signal that is set when the local node is done initializing all its
	/// protocols and is ready to accept connections from remote peers.
	ready_signal: SetOnce<()>,

	/// Cancellation token that is triggered when the running network instance is
	/// being shut down or has unrecoverable failure.
	termination: CancellationToken,
}
