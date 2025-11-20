use {
	crate::{local::Local, prelude::PeerId},
	core::fmt,
	iroh::{
		endpoint::Connection,
		protocol::{AcceptError, ProtocolHandler},
	},
	serde::{Deserialize, Serialize},
};

struct Protocol {
	local: Local,
}

impl Protocol {
	const ALPN: &'static [u8] = b"/mosaik/groups/1";

	fn new(local: Local) -> Self {
		Self { local }
	}
}

impl ProtocolHandler for Protocol {
	async fn accept(&self, _connection: Connection) -> Result<(), AcceptError> {
		todo!()
	}
}

impl fmt::Debug for Protocol {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "Groups({})", String::from_utf8_lossy(Self::ALPN))
	}
}

/// This message is sent when a node suspects that a peer has failed (e.g. due
/// to missed heartbeats on the persistent link) to all of the raft voters.
///
/// When raft voters observe more than N% of the network peers sending this
/// message about the same peer, then the suspect peer is removed from the
/// latest `GroupState` structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SuspectFail {
	peer: PeerId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PrepareJoin {}
