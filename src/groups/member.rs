use {
	crate::{PeerId, discovery::SignedPeerEntry, groups::log::Index},
	im::{HashMap, hashmap::Entry},
	itertools::Itertools,
	serde::{Deserialize, Serialize},
	tokio::sync::watch,
};

/// Replicated commands to modify the membership state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MembershipCommand {
	/// A new member has been added to the group or it updated its own info.
	InsertMember(Box<SignedPeerEntry>),

	/// A peer has been removed from the group.
	RemoveMember(PeerId),
}

/// Tracks the replicated membership state of a group.
///
/// This is the state machine that gets updated when Raft log entries are
/// committed. It maintains the authoritative list of group members that all
/// nodes should agree on.
///
/// The membership list is derived from committed log entries only, so it
/// represents the consensus view of the group, not the local view of active
/// bonds.
#[derive(Clone)]
pub struct Membership {
	/// The current set of group members, keyed by peer ID.
	/// Uses an immutable map for efficient cloning and iteration.
	members: watch::Sender<HashMap<PeerId, MemberInfo>>,
}

impl Default for Membership {
	fn default() -> Self {
		Self::new()
	}
}

/// Information about a group member stored in the membership state.
#[derive(Debug, Clone)]
pub struct MemberInfo {
	/// The signed peer entry for this member.
	pub info: SignedPeerEntry,

	/// The log index at which this member was added.
	pub added_at_index: Index,
}

/// Public API
impl Membership {
	pub fn new() -> Self {
		Self {
			members: watch::Sender::new(HashMap::new()),
		}
	}

	/// Returns the number of members in the group.
	pub fn len(&self) -> usize {
		self.members.borrow().len()
	}

	/// Returns `true` if the membership is empty.
	pub fn is_empty(&self) -> bool {
		self.members.borrow().is_empty()
	}

	/// Returns `true` if the given peer is a member of the group.
	pub fn contains(&self, peer_id: &PeerId) -> bool {
		self.members.borrow().contains_key(peer_id)
	}

	/// Returns the member info for the given peer, if they are a member.
	pub fn get(&self, peer_id: &PeerId) -> Option<SignedPeerEntry> {
		self
			.members
			.borrow()
			.get(peer_id)
			.cloned()
			.map(|info| info.info)
	}

	/// Returns an iterator over all member peer IDs.
	pub fn ids(&self) -> impl Iterator<Item = PeerId> {
		self.members.borrow().clone().into_iter().map(|(id, _)| id)
	}

	/// Returns an iterator over all member peer entries ordered by the time they
	/// were added to the group from oldest to most recent. Updates to an existing
	/// peer entry do not affect the order.
	pub fn iter(&self) -> impl Iterator<Item = SignedPeerEntry> {
		self
			.members
			.borrow()
			.clone()
			.into_iter()
			.sorted_by_key(|(_, info)| info.added_at_index)
			.map(|(_, info)| info.info)
	}

	/// Returns an iterator over the peer IDs of the voting members in the group.
	/// All other members are observers and don't vote in leadership elections.
	pub fn voters(&self) -> impl Iterator<Item = PeerId> {
		let members_count = self.members.borrow().len();
		let voting_size = voting_committee_size(members_count);
		self.iter().take(voting_size).map(|info| *info.id())
	}
}

/// Internal API
impl Membership {
	/// Applies a committed log command to update the membership state.
	///
	/// This should only be called when a log entry has been committed
	/// (replicated to a majority and safe to apply).
	pub(super) fn apply(&self, command: MembershipCommand, index: Index) {
		match command {
			MembershipCommand::InsertMember(member) => {
				self.members.send_if_modified(|members| {
					match members.entry(*member.id()) {
						Entry::Occupied(mut entry) => {
							if member.is_newer_than(&entry.get().info) {
								entry.get_mut().info = *member;
								true
							} else {
								false
							}
						}
						Entry::Vacant(entry) => {
							entry.insert(MemberInfo {
								info: *member,
								added_at_index: index,
							});
							true
						}
					}
				});
			}
			MembershipCommand::RemoveMember(peer_id) => {
				self
					.members
					.send_if_modified(|members| members.remove(&peer_id).is_some());
			}
		}
	}
}

/// Determines the voting committee size based on total group size. Additional
/// members become non-voting observers. Always round to the nearest odd number
/// for clear majorities.
fn voting_committee_size(group_size: usize) -> usize {
	const MAX_VOTING_MEMBERS: usize = 5;

	match group_size {
		0 => 0,
		1 | 2 => 1,
		_ => {
			let size = group_size.min(MAX_VOTING_MEMBERS);
			if size.is_multiple_of(2) {
				size - 1
			} else {
				size
			}
		}
	}
}
