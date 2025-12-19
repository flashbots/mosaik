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
		discovery::Discovery,
		groups::join::Join,
		network::{LocalNode, ProtocolProvider},
	},
	iroh::{EndpointId, PublicKey, protocol::RouterBuilder},
	serde::{Deserialize, Serialize},
	std::{collections::BTreeMap, sync::Arc},
	tokio::sync::RwLock,
};

mod config;
mod error;
mod join;

pub use {
	config::{Config, ConfigBuilder, ConfigBuilderError},
	error::Error,
};

use crate::network::link::Protocol as _;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GroupState {
	public_key: iroh::PublicKey,
	members: Vec<EndpointId>,
}

impl GroupState {
	pub fn new(public_key: iroh::PublicKey, members: Vec<EndpointId>) -> Self {
		Self {
			public_key,
			members,
		}
	}
}

pub struct Groups {
	local: LocalNode,
	discovery: Discovery,
	groups_to_join: Vec<config::Group>,
	group_states: BTreeMap<PublicKey, Arc<RwLock<GroupState>>>,
}

impl Groups {
	pub fn new(local: LocalNode, discovery: Discovery, config: Config) -> Self {
		let Config { groups_to_join } = config;
		let group_states = groups_to_join
			.iter()
			.map(|group| {
				(
					group.secret().public().clone(),
					Arc::new(RwLock::new(GroupState::new(
						group.secret().public().clone(),
						vec![],
					))),
				)
			})
			.collect::<BTreeMap<_, _>>();

		Self {
			local,
			discovery,
			groups_to_join,
			group_states,
		}
	}

	pub async fn run(self) {
		let Self {
			local,
			discovery,
			groups_to_join,
			group_states,
		} = self;

		loop {
			tokio::select! {
				_ = local.termination().cancelled() => {
					break;
				}
			}
		}
	}
}

impl ProtocolProvider for Groups {
	fn install(&self, protocols: RouterBuilder) -> RouterBuilder {
		protocols.accept(Join::ALPN, Join::new(self.group_states.clone()))
	}
}
