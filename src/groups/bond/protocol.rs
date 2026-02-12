use {
	crate::{
		Digest,
		NetworkId,
		discovery::{Discovery, SignedPeerEntry},
		groups::{
			Config,
			GroupId,
			Groups,
			error::{GroupNotFound, InvalidHandshake, Timeout},
			state::WorkerState,
		},
		network::{
			CloseReason,
			DifferentNetwork,
			LocalNode,
			UnknownPeer,
			link::{Link, Protocol},
		},
		primitives::Short,
	},
	bytes::Bytes,
	core::fmt,
	dashmap::DashMap,
	iroh::{
		endpoint::{ApplicationClose, Connection},
		protocol::{AcceptError, ProtocolHandler},
	},
	serde::{Deserialize, Serialize},
	std::sync::Arc,
	tokio::time::timeout,
};

/// This is the initial message sent by the peer initiating a connection on the
/// groups protocol to another member of the group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandshakeStart {
	/// The unique identifier of the network that the group belongs to.
	pub network_id: NetworkId,

	/// The unique identifier of the group that is derived from the group key.
	pub group_id: GroupId,

	/// A proof of knowledge of the secret group key by hashing the secret with
	/// the TLS-derived shared secret and the peer id.
	pub proof: Digest,

	/// A list of peers that the initiating node has formed bonds with but are
	/// not yet part of the group consensus. Those usually are nodes that are
	/// still in the process of joining the group and catching up with the
	/// latest state.
	pub bonds: Vec<SignedPeerEntry>,
}

/// This is the second message exchanged during the handshake process. The
/// accepting node responds to the initiator's challenge with its own nonce and
/// a response to the initiator's challenge, by hashing the secret with the
/// initiator's nonce.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandshakeEnd {
	/// A proof of knowledge of the secret group key by hashing the secret with
	/// the TLS-derived shared secret and the peer id.
	pub proof: Digest,

	/// A list of peers that the initiating node has formed bonds with but are
	/// not yet part of the group consensus. Those usually are nodes that are
	/// still in the process of joining the group and catching up with the
	/// latest state.
	pub bonds: Vec<SignedPeerEntry>,
}

/// Messages exchanged over an established bond connection between two group
/// members.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BondMessage {
	/// Bond-level liveness check.
	Ping,

	/// Response to a `Ping` message.
	Pong,

	/// Broadcasted to all bonded peers when the local peer's discovery entry
	/// changes to propagate the update quickly to all group members.
	PeerEntryUpdate(Box<SignedPeerEntry>),

	/// Broadcasted to all bonded peers when a new peer forms a bond with the
	/// local node.
	BondFormed(Box<SignedPeerEntry>),

	/// Messages that drive the Raft consensus protocol within the group.
	/// These are relayed to the raft protocol handler and are not interpreted at
	/// the bond level because at the bond level we don't care about the state
	/// machine implementation.
	Raft(Bytes),
}

/// Protocol Acceptor
///
/// This type is responsible for accepting incoming connections for the groups
/// protocol and routing them to the appropriate group instance running on the
/// local node.
///
/// The local node must be already joined to the group for the connection to be
/// accepted using [`Groups::join`]
pub(in crate::groups) struct Acceptor {
	local: LocalNode,
	config: Arc<Config>,
	discovery: Discovery,
	active: Arc<DashMap<GroupId, Arc<WorkerState>>>,
}

impl Acceptor {
	/// Create a new Acceptor instance that is the first point of contact for
	/// incoming connections on the groups ALPN protocol. The acceptor is
	/// responsible for performing the initial handshake and routing the
	/// connection to the appropriate group instance.
	pub fn new(groups: &Groups) -> Self {
		Self {
			local: groups.local.clone(),
			discovery: groups.discovery.clone(),
			config: Arc::clone(&groups.config),
			active: Arc::clone(&groups.active),
		}
	}
}

impl fmt::Debug for Acceptor {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		// Safety: ALPN is valid UTF-8 hardcoded at compile time
		unsafe { write!(f, "{}", str::from_utf8_unchecked(Groups::ALPN)) }
	}
}

impl ProtocolHandler for Acceptor {
	/// Invoked when a new incoming connection is established on the groups
	/// protocol. This method performs the handshake process and routes the
	/// connection to the appropriate group instance.
	async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
		let peer_id = connection.remote_id();

		// wrap the connection in a Link that speaks the Groups protocol
		let link = Link::<Groups>::accept_with_cancel(
			connection,
			self.local.termination().child_token(),
		)
		.await?;

		// ensure that the remote peer is known in our discovery catalog.
		let (link, peer) = self.ensure_known_peer(link).await?;

		// wait for the initiating peer to send the handshake start message.
		let (handshake, link) = self.wait_for_handshake(link).await?;

		// ensure that the initiating peer is connecting to the same network
		let link = self.ensure_same_network(link, &handshake).await?;

		// look up the group instance for the requested group id
		let Some(group) = self.active.get(&handshake.group_id) else {
			return Err(self.abort(link, GroupNotFound).await);
		};

		// hand off the established link to the group instance for management,
		// aborting if we are already connected to this peer in this group
		group
			.value()
			.accept(link, peer, handshake)
			.await
			.inspect_err(|e| {
				if !is_already_bonded_error(e) {
					tracing::trace!(
						error = ?e,
						peer = %Short(peer_id),
						network = %self.local.network_id(),
						"rejected bond connection",
					);
				}
			})
	}
}

// Supporting functions used during `accept` handshake process
impl Acceptor {
	/// The accepting node will await the initial handshake message from the
	/// initiating peer. If the message is not received within the configured
	/// timeout duration, an error is returned and the connection will be aborted.
	async fn wait_for_handshake(
		&self,
		mut link: Link<Groups>,
	) -> Result<(HandshakeStart, Link<Groups>), AcceptError> {
		let recv_fut = timeout(
			self.config.handshake_timeout, //
			link.recv::<HandshakeStart>(),
		);

		match recv_fut.await {
			Ok(Ok(start)) => Ok((start, link)),
			Ok(Err(e)) => {
				tracing::debug!(
					network = %self.local.network_id(),
					error = ?e,
					"group handshake receive error"
				);
				Err(self.abort(link, InvalidHandshake).await)
			}
			Err(_) => {
				tracing::trace!(
					network = %self.local.network_id(),
					peer = %Short(link.remote_id()),
					"group handshake timed out",
				);
				Err(self.abort(link, Timeout).await)
			}
		}
	}

	/// Ensures that the initiating peer is connecting to the same network as
	/// this node.
	async fn ensure_same_network(
		&self,
		link: Link<Groups>,
		start: &HandshakeStart,
	) -> Result<Link<Groups>, AcceptError> {
		if start.network_id != *self.local.network_id() {
			tracing::debug!(
				network = %self.local.network_id(),
				peer = %Short(link.remote_id()),
				expected_network = %Short(self.local.network_id()),
				received_network = %Short(start.network_id),
				"peer connected to wrong network",
			);

			return Err(self.abort(link, DifferentNetwork).await);
		}

		Ok(link)
	}

	/// Ensures that the remote peer is known in our discovery catalog. If the
	/// peer is not known, the connection is aborted.
	async fn ensure_known_peer(
		&self,
		link: Link<Groups>,
	) -> Result<(Link<Groups>, SignedPeerEntry), AcceptError> {
		let Some(peer) = self
			.discovery
			.catalog()
			.get_signed(&link.remote_id())
			.cloned()
		else {
			tracing::trace!(
				network = %self.local.network_id(),
				peer = %Short(&link.remote_id()),
				"rejecting unknown peer",
			);
			return Err(self.abort(link, UnknownPeer).await);
		};

		Ok((link, peer))
	}

	/// Terminates an incoming connection during the handshake process due to an
	/// error. This closes the link with the remote peer using the provided
	/// application-level close reason and returns an `AcceptError` that can be
	/// returned directly from the `accept` method.
	async fn abort(
		&self,
		link: Link<Groups>,
		reason: impl CloseReason,
	) -> AcceptError {
		let remote_id = link.remote_id();
		let app_reason: ApplicationClose = reason.clone().into();
		if let Err(e) = link.close(app_reason.clone()).await {
			tracing::debug!(
				network = %self.local.network_id(),
				peer = %Short(remote_id),
				error = %e,
				"failed to close link during handshake abort",
			);
			return AcceptError::from_err(e);
		}

		AcceptError::from_err(reason)
	}
}

// used to reduce noise in logs for benign "already bonded" errors during the
// bonding process.
#[inline]
fn is_already_bonded_error(e: &AcceptError) -> bool {
	e.to_string() == "AlreadyBonded"
}
