use {
	crate::{local::Local, prelude::PeerId},
	core::fmt,
	iroh::{
		endpoint::Connection,
		protocol::{AcceptError, ProtocolHandler},
	},
	serde::{Deserialize, Serialize},
};

pub struct Protocol {
	local: Local,
}

impl Protocol {
	pub const ALPN: &'static [u8] = b"/mosaik/groups/1";

	pub fn new(local: Local) -> Self {
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
pub struct SuspectFail {
	pub peer: PeerId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrepareJoin {
  
}
