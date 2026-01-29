use {
	super::{Bond, BondHandle, Error},
	crate::{
		Groups,
		discovery::PeerEntry,
		groups::{
			InvalidProof,
			accept::{HandshakeEnd, HandshakeStart},
			group::GroupState,
		},
		network::{UnknownPeer, link::Link},
		primitives::Short,
	},
	std::sync::Arc,
};

impl Bond {
	/// Initiates the process of creating a new bond connection to a remote
	/// peer in the group.
	///
	/// This happens in response to discovering a new peer in the group via
	/// the discovery catalog. This method is called only for peers that are
	/// already known in the discovery catalog.
	pub async fn create(
		group: Arc<GroupState>,
		peer: PeerEntry,
	) -> Result<BondHandle, Error> {
		tracing::trace!(
			network = %group.local.network_id(),
			peer = %Short(peer.id()),
			group = %Short(group.key.id()),
			"initiating peer bond",
		);

		// attempt to establish a new wire link to the remote peer
		let mut link = group
			.local
			.connect_with_cancel::<Groups>(
				peer.address().clone(),
				group.cancel.child_token(),
			)
			.await
			.map_err(|e| Error::Link(e.into()))?;

		// prepare a handshake message with our proof of knowledge of the group
		// secret and send it to the remote peer over the newly established link
		link
			.send(&HandshakeStart {
				network_id: *group.local.network_id(),
				group_id: *group.key.id(),
				proof: group.key.generate_proof(&link, group.local.id()),
			})
			.await
			.map_err(|e| Error::Link(e.into()))?;

		// After sending our handshake with a proof of knowledge of the group
		// secret, wait for the accepting peer to respond with its own proof of
		// knowledge of the group secret.
		let confirm = match link.recv::<HandshakeEnd>().await {
			Ok(resp) => resp,
			Err(e) => match e.close_reason() {
				// the remote peer closed the link during handshake because the local
				// node is not known in its discovery catalog. Trigger full discovery
				// catalog sync.
				Some(reason) if reason == UnknownPeer => {
					// trigger full catalog sync and retry bonding
					group
						.discovery
						.sync_with(peer.address().clone())
						.await
						.map_err(Error::Discovery)?;

					// retry creating the bond after syncing the catalog
					return Box::pin(Self::create(group, peer)).await;
				}
				// The remote peer rejected our authentication proof.
				// This is an unrecoverable error in the current set of authorization
				// schemes and most likely indicates an incompatible version of the peer
				// or a malicious actor.
				Some(reason) if reason == InvalidProof => {
					todo!()
				}
				// Bonding failed for some other reason.
				_ => return Err(Error::Link(e.into())),
			},
		};

		// validate the accepting peer's proof of knowledge of the group secret
		if !group.key.validate_proof(&link, confirm.proof) {
			tracing::warn!(
				network = %group.local.network_id(),
				peer = %Short(peer.id()),
				group = %Short(group.key.id()),
				"remote peer provided invalid group secret proof",
			);

			link
				.close(InvalidProof)
				.await
				.map_err(|e| Error::Link(e.into()))?;
			return Err(Error::InvalidProof);
		}

		Ok(Bond::spawn(group, peer, link))
	}

	/// Accepts an incoming bond connection for this group.
	///
	/// This is called by the group's protocol handler when a new connection
	/// is established  in [`Listener::accept`].
	///
	/// By the time this method is called:
	/// - The network id has already been verified to match the local node's
	///   network id.
	/// - The group id has already been verified to match this group's id.
	/// - The presence of the remote peer in the local discovery catalog is
	///   verified.
	/// - The authentication proof has not been verified yet.
	pub async fn accept(
		group: Arc<GroupState>,
		link: Link<Groups>,
		peer: PeerEntry,
		handshake: HandshakeStart,
	) -> Result<BondHandle, Error> {
		let mut link = link;

		// verify the remote peer's proof of knowledge of the group secret
		if !group.key.validate_proof(&link, handshake.proof) {
			tracing::warn!(
				network = %group.local.network_id(),
				peer = %Short(peer.id()),
				group = %Short(group.key.id()),
				"remote peer provided invalid group secret proof",
			);

			link
				.close(InvalidProof)
				.await
				.map_err(|e| Error::Link(e.into()))?;

			return Err(Error::InvalidProof);
		}

		// After verifying the remote peer's proof of knowledge of the group secret,
		// respond with our own proof of knowledge of the group secret.
		let proof = group.key.generate_proof(&link, group.local.id());
		let resp = HandshakeEnd { proof };
		link.send(&resp).await.map_err(|e| Error::Link(e.into()))?;

		Ok(Bond::spawn(group, peer, link))
	}
}
