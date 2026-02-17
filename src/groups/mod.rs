//! # Availability Groups
//!
//! Availability groups are clusters of trusted nodes on the same mosaik network
//! that coordinate with each other for load balancing and failover. Members of
//! a group share a secret key that authenticates membership, and they maintain
//! a consistent, replicated view of group state through a modified Raft
//! consensus protocol.
//!
//! ## Trust Model
//!
//! Groups are **not** Byzantine fault tolerant. All members within a group are
//! assumed to be honest and operated by the same entity. The group key acts as
//! the sole admission control — only nodes that know the key can join.
//!
//! ## Bonds
//!
//! Every pair of group members maintains a persistent **bond** — an
//! authenticated, bidirectional connection established through a mutual secret
//! proof exchange. Bonds carry Raft consensus messages, heartbeats, and
//! log-sync traffic. The full mesh of bonds gives every member a direct channel
//! to every other member.
//!
//! ## Consensus
//!
//! Groups run a modified Raft protocol to elect a leader and replicate a log of
//! commands to a pluggable [`StateMachine`]. Key differences from standard
//! Raft:
//!
//! - **Non-voting followers.** A follower whose log is behind the leader's
//!   state is considered a non-voting follower. Non-voting followers send
//!   `Abstain` responses to both `RequestVote` and `AppendEntries` messages.
//!   They automatically become voting members once their log catches up.
//!
//! - **Leader simplicity.** The leader does not track per-follower progress
//!   (`next_index` / `match_index`). It broadcasts `AppendEntries` with the
//!   latest entries and advances its commit index based on the count of
//!   affirmative acknowledgements, ignoring abstentions.
//!
//! - **Dynamic quorum.** Abstaining (out-of-sync) followers are excluded from
//!   the quorum denominator for both elections and commit advancement, so
//!   consensus can proceed while lagging nodes catch up.
//!
//! - **Distributed log catch-up.** A lagging follower recovers missing log
//!   entries by broadcasting a `LogSyncDiscovery` to all bonded peers,
//!   collecting availability responses, partitioning the needed range across
//!   responders for balanced load, and pulling entries in parallel. Incoming
//!   `AppendEntries` are buffered during catch-up and applied once the gap is
//!   closed.
//!
//! ## Group Identity
//!
//! A [`GroupId`] is derived from the group key, the consensus-relevant
//! configuration (election timeouts, heartbeat intervals, etc.), and the
//! replicated state machine's identifier. Any divergence in these values
//! produces a different group id, preventing misconfigured nodes from bonding.
//!
//! ## Usage
//!
//! ```ignore
//! // join a group with a specific key and default configuration
//! let group = network.groups().with_key(key).join();
//!
//! // Wait for a leader to be elected
//! group.when().leader_elected().await;
//! ```

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
mod builder;
mod config;
mod error;
mod group;
mod key;
mod log;
mod machine;
mod raft;
mod replay;
mod when;

pub use {
	bond::{Bond, Bonds},
	builder::{
		ConsensusConfig,
		ConsensusConfigBuilder,
		ConsensusConfigBuilderError,
		GroupBuilder,
	},
	config::{Config, ConfigBuilder, ConfigBuilderError},
	error::{CommandError, Error, QueryError},
	group::*,
	key::GroupKey,
	log::{Cursor, InMemoryLogStore, Index, IndexRange, Storage, Term},
	machine::*,
	replay::*,
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
	pub const fn with_key(&self, key: GroupKey) -> GroupBuilder<'_> {
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
