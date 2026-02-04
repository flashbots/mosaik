use {
	crate::{
		NetworkId,
		UniqueId,
		discovery::SignedPeerEntry,
		groups::{GroupId, consensus::ConsensusMessage},
	},
	serde::{Deserialize, Serialize},
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
	pub proof: UniqueId,

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
	pub proof: UniqueId,

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
	Consensus(ConsensusMessage),
}
