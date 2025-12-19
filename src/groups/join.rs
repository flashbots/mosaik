use {
	crate::{
		groups::{Error, GroupState},
		network::link::Link,
	},
	core::fmt,
	iroh::{
		EndpointAddr,
		PublicKey,
		Signature,
		endpoint::Connection,
		protocol::{AcceptError, ProtocolHandler},
	},
	serde::{Deserialize, Serialize},
	std::{collections::BTreeMap, str, sync::Arc},
	tokio::sync::RwLock,
};

/// Arbitrary message which must be signed by the group private key
/// to request joining a group.
pub(crate) const JOIN_GROUP_MESSAGE: &[u8] = b"join_request";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Request {
	// Signature over JOIN_GROUP_MESSAGE using the group private key
	signature: bytes::Bytes,

	// The group public key, which is its identifier
	public_key: PublicKey,
}

/// Protocol handler for the `join` protocol, which allows nodes to join
/// existing groups.
pub(super) struct Join {
	group_states: BTreeMap<PublicKey, Arc<RwLock<GroupState>>>,
}

impl Join {
	/// ALPN identifier for the groups join protocol.
	pub(super) const ALPN: &'static [u8] = b"/mosaik/groups/join/1";

	pub(super) fn new(
		group_states: BTreeMap<PublicKey, Arc<RwLock<GroupState>>>,
	) -> Self {
		Self { group_states }
	}
}

impl fmt::Debug for Join {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		// Safety: ALPN is valid UTF-8 hardcoded at compile time
		unsafe { write!(f, "{}", str::from_utf8_unchecked(Self::ALPN)) }
	}
}

impl ProtocolHandler for Join {
	async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
		let mut link = Link::accept(connection, Join::ALPN).await?;
		let msg = link
			.recv::<Request>()
			.await
			.map_err(AcceptError::from_err)?;

		// check that public key is in a known group
		let group_state: &Arc<RwLock<GroupState>> =
			if let Some(state) = self.group_states.get(&msg.public_key) {
				state
			} else {
				return Err(AcceptError::from_err(Error::UnknownGroupPublicKey));
			};

		// verify signature
		let signature_bytes: [u8; 64] =
			msg.signature.as_ref().try_into().map_err(|_| {
				AcceptError::from_err(Error::InvalidSignatureLength(
					msg.signature.len(),
				))
			})?;
		let signature = Signature::from_bytes(&signature_bytes);
		if let Err(e) = msg.public_key.verify(JOIN_GROUP_MESSAGE, &signature) {
			return Err(AcceptError::from_err(e));
		}

		// join request is valid; attempt to add peer to group.
		// 1. send current group state to peer
		// 2. add remote peer to local `GroupState`
		// 3. notify RAFT leaders of new member (TODO)
		{
			let group_state = group_state.read().await;
			link
				.send(&group_state.clone())
				.await
				.map_err(AcceptError::from_err)?;
		}

		let mut state = group_state.write().await;
		state.push_member(link.remote_id());

		Ok(())
	}
}

pub(super) async fn join_group(
	local: crate::network::LocalNode,
	discovery: crate::discovery::Discovery,
	group: crate::groups::config::Group,
	group_state: Arc<RwLock<GroupState>>,
) -> Result<(), Error> {
	use futures::{StreamExt as _, stream::FuturesUnordered};

	let catalog = discovery.catalog();
	let mut futs = FuturesUnordered::new();
	for remote in group.known_members() {
		let Some(peer) = catalog.get(remote) else {
			continue;
		};

		let group = group.clone();
		let local = local.clone();
		let group_state = group_state.clone();
		futs.push(async move {
			let remote_group_state =
				join_group_with_remote(local.clone(), group, peer.address()).await?;
			{
				let mut local_state = group_state.write().await;
				*local_state = remote_group_state;
				local_state.push_member(local.id());
			}
			Ok::<(), Error>(())
		})
	}

	while let Some(res) = futs.next().await {
		match res {
			Ok(_) => {
				// we've successfully joined the group,
				// no need to try other members
				drop(futs);
				break;
			}
			Err(e) => {
				tracing::warn!(%e, "failed to join group via one of the known members");
			}
		}
	}

	Ok(())
}

async fn join_group_with_remote(
	local: crate::network::LocalNode,
	group: crate::groups::config::Group,
	remote: &EndpointAddr,
) -> Result<GroupState, Error> {
	let mut link = Link::open_with_cancel(
		&local,
		remote.clone(),
		Join::ALPN,
		local.termination().clone(),
	)
	.await
	.map_err(Error::OpenLinkFailed)?;

	link
		.send(&Request {
			signature: group
				.secret()
				.sign(JOIN_GROUP_MESSAGE)
				.to_bytes()
				.to_vec()
				.into(),
			public_key: group.secret().public(),
		})
		.await
		.map_err(Error::JoinRequestFailed)?;

	let remote_group_state = link
		.recv::<GroupState>()
		.await
		.map_err(Error::ReceiveGroupStateFailed)?;

	Ok(remote_group_state)
}
