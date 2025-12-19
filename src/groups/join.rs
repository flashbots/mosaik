use {
	crate::{
		groups::{Error, GroupState},
		network::link::{Link, Protocol},
	},
	core::fmt,
	iroh::{
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
	signature: Vec<u8>,

	// The group public key, which is its identifier
	public_key: PublicKey,
}

/// Protocol handler for the `join` protocol, which allows nodes to join
/// existing groups.
pub(super) struct Join {
	group_states: BTreeMap<PublicKey, Arc<RwLock<GroupState>>>,
}

impl Join {
	pub(super) fn new(
		group_states: BTreeMap<PublicKey, Arc<RwLock<GroupState>>>,
	) -> Self {
		Self { group_states }
	}
}

impl Protocol for Join {
	/// ALPN identifier for the groups join protocol.
	const ALPN: &'static [u8] = b"/mosaik/groups/join/1";
}

impl fmt::Debug for Join {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		// Safety: ALPN is valid UTF-8 hardcoded at compile time
		unsafe { write!(f, "{}", str::from_utf8_unchecked(Self::ALPN)) }
	}
}

impl ProtocolHandler for Join {
	async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
		let mut link = Link::<Join>::accept(connection).await?;
		let msg = link
			.recv::<Request>()
			.await
			.map_err(AcceptError::from_err)?;

		// check that public key is in a known group
		let group_state =
			if let Some(state) = self.group_states.get(&msg.public_key) {
				state.clone()
			} else {
				return Err(AcceptError::from_err(Error::UnknownGroupPublicKey));
			};

		// verify signature
		let signature_bytes: [u8; 64] =
			msg.signature.as_slice().try_into().map_err(|_| {
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
		state.members.push(link.remote_id());

		Ok(())
	}
}
