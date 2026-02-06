use {
	crate::{
		NetworkId,
		PeerId,
		groups::{
			GroupId,
			consensus::protocol::ReplicatedCommand,
			group::GroupState,
			log::{Log, Term},
		},
	},
	std::sync::Arc,
};

/// State that is shared across all raft roles.
pub struct Shared {
	/// The group state that this consensus instance is managing.
	group: Arc<GroupState>,

	/// The persistent log for this group that tracks all changes to the group's
	/// replicated state machine through raft.
	log: Log<ReplicatedCommand>,

	/// The last vote casted by the local node in leader elections.
	last_vote: Option<(Term, PeerId)>,
}

impl Shared {
	pub(super) fn new(group: Arc<GroupState>) -> Self {
		Self {
			group,
			log: Log::new(),
			last_vote: None,
		}
	}

	/// Reference to the group state that this consensus instance is managing.
	pub fn group(&self) -> &GroupState {
		&self.group
	}

	/// Peer ID of the local node.
	pub fn local_id(&self) -> PeerId {
		self.group.local.id()
	}

	/// Group ID of this consensus group.
	pub fn group_id(&self) -> GroupId {
		*self.group.key.id()
	}

	pub fn network_id(&self) -> NetworkId {
		*self.group.local.network_id()
	}
}
