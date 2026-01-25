use {
	crate::{
		NetworkId,
		UniqueId,
		groups::{
			AlreadyConnected,
			Config,
			Group,
			GroupId,
			GroupNotFound,
			Groups,
			HandshakeTimeout,
			InvalidAuth,
			InvalidHandshake,
		},
		network::{
			CloseReason,
			DifferentNetwork,
			LocalNode,
			link::{Link, Protocol},
		},
		primitives::Short,
	},
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

/// Protocol Acceptor
///
/// This type is responsible for accepting incoming connections for the groups
/// protocol and routing them to the appropriate group instance running on the
/// local node.
///
/// The local node must be already joined to the group for the connection to be
/// accepted using [`Groups::join`]
pub(super) struct Listener {
	local: LocalNode,
	config: Arc<Config>,
	active: Arc<DashMap<GroupId, Group>>,
}

impl Listener {
	/// Create a new Listener
	pub(super) fn new(groups: &Groups) -> Self {
		Self {
			local: groups.local.clone(),
			config: Arc::clone(&groups.config),
			active: Arc::clone(&groups.active),
		}
	}
}

impl fmt::Debug for Listener {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		// Safety: ALPN is valid UTF-8 hardcoded at compile time
		unsafe { write!(f, "{}", str::from_utf8_unchecked(Groups::ALPN)) }
	}
}

impl ProtocolHandler for Listener {
	async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
		// wrap the connection in a Link that speaks the Groups protocol
		let link = Link::<Groups>::accept_with_cancel(
			connection,
			self.local.termination().child_token(),
		)
		.await?;

		// wait for the initiating peer to send the handshake start message.
		let (start, link) = self.wait_for_handshake(link).await?;

		// ensure that the initiating peer is connecting to the same network
		let link = self.ensure_same_network(link, &start).await?;

		// look up the group instance for the requested group id
		let Some(group) = self.active.get(&start.group_id) else {
			return Err(self.abort(link, GroupNotFound).await);
		};

		// ensure that we are not already connected to this peer in this group
		let link = self.ensure_not_connected(&group, link).await?;

		// verify the the proof of knowledge of the secret provided in the handshake
		let link = self.validate_auth(group.value(), link, start).await?;

		// complete the handshake by sending our own proof of knowledge of the
		// secret
		let link = self.complete_handshake(group.value(), link).await?;

		// hand off the established link to the group instance for management,
		// aborting if we are already connected to this peer in this group
		match group.value().accept(link) {
			Ok(()) => Ok(()),
			Err((link, e)) => Err(self.abort(link, e).await),
		}
	}
}

// Supporting functions used during `accept` handshake process
impl Listener {
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
			Ok(Ok(start)) => {
				tracing::trace!(
					network = %self.local.network_id(),
					peer = %Short(link.remote_id()),
					group = %start.group_id,
					"group handshake received",
				);

				Ok((start, link))
			}
			Ok(Err(e)) => {
				tracing::debug!(
					network = %self.local.network_id(),
					error = ?e,
					"group handshake receive error"
				);
				Err(self.abort(link, InvalidHandshake).await)
			}
			Err(_) => {
				tracing::debug!(
					network = %self.local.network_id(),
					peer = %Short(link.remote_id()),
					"group handshake timed out",
				);
				Err(self.abort(link, HandshakeTimeout).await)
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

	/// Ensures that the initiating peer is not already connected to this node in
	/// this group.
	///
	/// There is no predefined logic that decides which peer should initiate the
	/// connection and which should accept it. The first peer that discovers the
	/// other and initiates the connection will be accepted, while the other
	/// peer's attempt will be rejected.
	async fn ensure_not_connected(
		&self,
		group: &Group,
		link: Link<Groups>,
	) -> Result<Link<Groups>, AcceptError> {
		if group.has_member(&link.remote_id()) {
			tracing::trace!(
				network = %self.local.network_id(),
				peer = %Short(link.remote_id()),
				group = %Short(group.id()),
				"duplicate group connection attempt",
			);

			return Err(self.abort(link, AlreadyConnected).await);
		}

		Ok(link)
	}

	/// Validates the authentication proof of knowledge of the group secret that
	/// is sent by the initiating peer during the handshake process.
	async fn validate_auth(
		&self,
		group: &Group,
		link: Link<Groups>,
		start: HandshakeStart,
	) -> Result<Link<Groups>, AcceptError> {
		let expected_auth = group
			.key()
			.secret()
			.derive(link.shared_random(start.group_id))
			.derive(link.remote_id().as_bytes());

		if start.auth != expected_auth {
			tracing::warn!(
				network = %self.local.network_id(),
				peer = %Short(link.remote_id()),
				group = %start.group_id,
				"invalid group authentication attempt",
			);

			return Err(self.abort(link, InvalidAuth).await);
		}

		Ok(link)
	}

	/// After successfully validating the handshake from the initiating peer, the
	/// accepting peer also sends its proof of knowledge of the group secret back
	/// to the initiator and signals that a link has been successfully
	/// established.
	async fn complete_handshake(
		&self,
		group: &Group,
		mut link: Link<Groups>,
	) -> Result<Link<Groups>, AcceptError> {
		let auth = group
			.key()
			.secret()
			.derive(link.shared_random(group.id()))
			.derive(self.local.id().as_bytes());

		link
			.send(&HandshakeEnd { auth })
			.await
			.inspect_err(|e| {
				tracing::debug!(
					network = %self.local.network_id(),
					peer = %Short(link.remote_id()),
					group = %Short(group.id()),
					error = %e,
					"failed to send handshake response",
				);
			})
			.map_err(AcceptError::from_err)?;

		Ok(link)
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
	pub auth: UniqueId,
}

/// This is the second message exchanged during the handshake process. The
/// accepting node responds to the initiator's challenge with its own nonce and
/// a response to the initiator's challenge, by hashing the secret with the
/// initiator's nonce.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandshakeEnd {
	/// A proof of knowledge of the secret group key by hashing the secret with
	/// the TLS-derived shared secret and the peer id.
	pub auth: UniqueId,
}
