//! Availability Groups
//!
//! Availability groups are formed by nodes on the same mosaik network that work
//! together. The are aware and of each other and are managed by the same
//! entity. They have trust assumptions about each other and assume that they
//! are all honest and follow the protocol within the same group. Groups are
//! authenticated and only peers that know the seed bytes are allowed to join a
//! group.
//!
//! Notes:
//!
//! - Members of a one group have trust assumptions. Groups are not byzantine
//!   failures tolerant.
//!
//! - Nodes can form trusted availability groups for load balancing purposes.
//!
//! - Each availability group maintains a consistent list of all group members.
//!   The consistent view is managed by raft. In an availability group a small
//!   subset of nodes is elected to be voters in raft (1-5 nodes) and all other
//!   nodes are observers of the latest state of the group.
//!   - For groups of size 1, the raft voting committee size is 1
//!   - For groups of size 2, the raft voting committee size is 1
//!   - For  groups of size 3+, the raft voting committee is fixed as 3.
//!   - If the group redundancy config value is greater than 3, then that value
//!     becomes the size of the voting committee rounded up to the nearest odd
//!     number.
//!
//! - Availability groups maintain a `GroupState` data structure that lists all
//!   known nodes in the group. This structure is updated and consensus is
//!   reached by the group leaders whenever a node failure is detected or a new
//!   node joins the group.
//!
//! - All nodes within one availability group maintain all-to-all persistent
//!   connections with frequent short health checks. As soon as a node failure
//!   is detected, then all nodes will send `SuspectFail` message to all known
//!   leaders.
//!
//! - When leaders receive N `SuspectFail` messages about a peer, then they will
//!   remove that peer from the most recent `GroupState` and come to consensus
//!   about the new version of `GroupState`.
//!
//! - When a new node wants to join a group, it will:
//!   - send a `DescribeGroup` message to any member of the group.
//!     - in response it will receive the latest `GroupState` of the group
//!   - After knowing about the latest list of peers in the group it will
//!     establish a persistent connection with each peer in the group and send a
//!     `PrepareJoin` message.
//!   - Existing group members will accept the persistent connection and
//!     maintain and send a `LinkEstablished` message to all current raft
//!     leaders. If raft leaders do not generate a new view of the `GroupState`
//!     structure that include this new node within a predefined short timeout,
//!     then the persistent connection is dropped. Otherwise the persistent
//!     connection is maintained and the new node becomes a member of the
//!     availability group.

use {
	crate::{
		UniqueId,
		discovery::Discovery,
		network::{self, LocalNode, ProtocolProvider, link::Protocol},
	},
	accept::Listener,
	dashmap::{DashMap, Entry},
	iroh::protocol::RouterBuilder,
	std::sync::Arc,
};

mod accept;
mod config;
mod error;
mod group;
mod key;
mod status;

pub use {
	config::{Config, ConfigBuilder, ConfigBuilderError},
	error::Error,
	group::Group,
	key::GroupKey,
};

/// A unique identifier for a group that is derived from the group key.
pub type GroupId = UniqueId;

pub struct Groups {
	local: LocalNode,
	config: Arc<Config>,
	discovery: Discovery,
	active: Arc<DashMap<GroupId, Group>>,
}

/// Public API
impl Groups {
	pub fn join(&self, group_key: GroupKey) -> Result<Group, Error> {
		let id = *group_key.id();
		match self.active.entry(id) {
			Entry::Occupied(entry) => Ok(entry.get().clone()),
			Entry::Vacant(entry) => {
				let group = Group::new(self, group_key);
				let group = entry.insert(group).clone();

				self
					.discovery
					.update_local_entry(move |entry| entry.add_groups(id));

				Ok(group)
			}
		}
	}
}

impl Groups {
	pub fn new(local: LocalNode, discovery: &Discovery, config: Config) -> Self {
		let config = Arc::new(config);

		Self {
			local,
			config,
			discovery: discovery.clone(),
			active: Arc::new(DashMap::new()),
		}
	}
}

impl Protocol for Groups {
	/// ALPN identifier for the groups protocol.
	const ALPN: &'static [u8] = b"/mosaik/groups/1";
}

impl ProtocolProvider for Groups {
	fn install(&self, protocols: RouterBuilder) -> RouterBuilder {
		protocols.accept(Self::ALPN, Listener::new(self))
	}
}

network::make_close_reason!(
	/// An error occurred during the handshake receive or decode process.
	struct InvalidHandshake, 30_400);

network::make_close_reason!(
	/// The group id specified in the handshake is not known to the accepting node.
	struct GroupNotFound, 30_404);

network::make_close_reason!(
	/// The handshake process timed out while waiting for a message from the remote peer.
	struct HandshakeTimeout, 30_408);

network::make_close_reason!(
	/// The authentication proof provided in the handshake is invalid.
	struct InvalidAuth, 30_405);

network::make_close_reason!(
	/// A link between those two peers in the same group already exists.
	struct AlreadyBonded, 30_429);
