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
//! - Nodes can form trusted availability groups for load balancing and/or
//!   failover purposes.
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
		Digest,
		discovery::Discovery,
		groups::state::WorkerState,
		network::{LocalNode, ProtocolProvider, link::Protocol},
	},
	dashmap::DashMap,
	iroh::protocol::RouterBuilder,
	std::sync::Arc,
};

mod bond;
mod config;
mod consensus;
mod error;
mod group;
mod key;
mod log;
mod when;

pub use {
	bond::{Bond, Bonds},
	config::{Config, ConfigBuilder, ConfigBuilderError},
	error::Error,
	group::*,
	key::GroupKey,
	log::*,
	when::When,
};

/// A unique identifier for a group that is derived from:
///
/// - The group key
/// - The group consensus-relevant configuration values, such as election
///   timeouts and heartbeat intervals.
/// - The id of the replicated state machine that is used by the raft replicated
///   log.
///
/// Any difference in any of the above values will result in a different group
/// id and will prevent the nodes from forming a bond connection with each
/// other.
pub type GroupId = Digest;

/// Public API gateway for the Groups subsystem.
///
/// This type is instantiated once per `Network` instance and is used to join
/// groups and manage them.
pub struct Groups {
	/// Reference to the local node networking stack instance.
	///
	/// This is needed to:
	/// - Initiate outgoing connections to other peers in the group when they are
	///   discovered and bonds need to be created.
	/// - Know the local peer id
	/// - Bind the cancellation token of groups workers to the lifecycle of the
	///   local node.
	local: LocalNode,

	/// Global configuration settings for the groups subsystem. This includes
	/// settings that are not specific to any group but affect the behavior of
	/// the subsystem as a whole, such as bonds handshake timeouts.
	config: Arc<Config>,

	/// A reference to the discovery service for peer discovery and peers
	/// catalog. A clone of this handle is passed to each group worker.
	discovery: Discovery,

	/// A map of all active groups that are joined by this node.
	///
	/// Each entry in this map corresponds to a unique group id and contains a
	/// handle to the worker loop that manages the state of that group. This map
	/// is used to ensure that only one worker loop is spawned for each group id,
	/// and to route incoming bond connection attempts to the correct worker loop
	/// based on the group id that is derived from the bond request.
	active: Arc<DashMap<GroupId, Arc<WorkerState>>>,
}

/// Public API
impl Groups {
	/// Returns a builder for configuring and joining a group with the specified
	/// group key.
	///
	/// The group id that will be generated by the builder and joined will be
	/// derived according to the rules described in the `GroupId` type
	/// definition. All members of the group must use identical configuration
	/// values for all the consensus-relevant parameters, otherwise they will
	/// generate different group ids and will not be able to form a bond
	/// connection with each other.
	pub fn with_key(&self, key: GroupKey) -> GroupBuilder<'_> {
		GroupBuilder::new(self, key)
	}
}

// Internal APIs
impl Groups {
	/// Creates a new instance of the `Groups` subsystem.
	///
	/// This is called once when the `Network` is being initialized, and the
	/// returned handle is stored within the `Network` struct and shared with all
	/// its child components.
	pub(crate) fn new(
		local: LocalNode,
		discovery: &Discovery,
		config: Config,
	) -> Self {
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
		protocols.accept(Self::ALPN, bond::Acceptor::new(self))
	}
}
