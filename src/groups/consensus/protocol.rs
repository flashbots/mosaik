use {
	crate::{
		PeerId,
		discovery::SignedPeerEntry,
		groups::log::{Index, LogEntry, Term},
		primitives::Short,
	},
	derive_more::Display,
	serde::{Deserialize, Serialize},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConsensusMessage {
	/// Sent by leaders to assert authority (heartbeat) and replicate log
	/// entries. When `entries` is empty, this is a pure heartbeat.
	AppendEntries(AppendEntries<ReplicatedCommand>),

	/// Response to an `AppendEntries` message.
	AppendEntriesResponse(AppendEntriesResponse),

	/// Sent by candidates to gather votes during an election.
	RequestVote(RequestVote),

	/// Response to a `RequestVote` message.
	RequestVoteResponse(RequestVoteResponse),
}

impl ConsensusMessage {
	/// Returns the term carried by the message.
	/// All raft messages include the sender's current term.
	pub fn term(&self) -> Term {
		match self {
			ConsensusMessage::AppendEntries(msg) => msg.term,
			ConsensusMessage::AppendEntriesResponse(msg) => msg.term,
			ConsensusMessage::RequestVote(msg) => msg.term,
			ConsensusMessage::RequestVoteResponse(msg) => msg.term,
		}
	}

	/// If the message was sent by a leader, returns its peer ID.
	pub fn leader(&self) -> Option<PeerId> {
		match self {
			ConsensusMessage::AppendEntries(msg) => Some(msg.leader),
			ConsensusMessage::AppendEntriesResponse(_)
			| ConsensusMessage::RequestVote(_)
			| ConsensusMessage::RequestVoteResponse(_) => None,
		}
	}
}

impl From<RequestVote> for ConsensusMessage {
	fn from(val: RequestVote) -> Self {
		ConsensusMessage::RequestVote(val)
	}
}

/// `RequestVote` Message arguments.
#[derive(Debug, Clone, Display, Serialize, Deserialize)]
#[display("{}[t{term}/{last_log_term}/{last_log_index}]", Short(candidate))]
pub struct RequestVote {
	/// Candidate's term.
	pub term: Term,

	/// Candidate requesting vote.
	pub candidate: PeerId,

	/// Index of candidate's last log entry (for log comparison).
	pub last_log_index: u64,

	/// Term of candidate's last log entry.
	pub last_log_term: Term,
}

/// `RequestVote` Message response.
#[derive(Debug, Clone, Display, Serialize, Deserialize)]
#[display("{vote_granted}@{term}")]
pub struct RequestVoteResponse {
	/// Current term, for candidate to update itself.
	pub term: Term,

	/// True means candidate received vote.
	pub vote_granted: bool,
}

/// `AppendEntries` message arguments.
///
/// Sent by leader to replicate log entries and as heartbeat.
/// When `entries` is empty, this serves as a heartbeat to maintain leadership.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppendEntries<C> {
	/// Leader's term.
	pub term: Term,

	/// Leader's peer ID, so followers can redirect clients.
	pub leader: PeerId,

	/// Index of log entry immediately preceding new ones.
	pub prev_log_index: Index,

	/// Term of `prev_log_index` entry.
	pub prev_log_term: Term,

	/// Log entries to store (empty for heartbeat).
	pub entries: Vec<LogEntry<C>>,

	/// Leader's commit index.
	pub leader_commit: Index,
}

/// `AppendEntries` message response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppendEntriesResponse {
	/// Current term, for leader to update itself.
	pub term: Term,

	/// True if follower contained entry matching `prev_log_index` and
	/// `prev_log_term`.
	pub success: bool,

	/// The responder's peer ID.
	pub responder_id: PeerId,

	/// Hint for leader to quickly find the correct `next_index` on failure.
	/// This is the follower's last log index.
	pub last_log_index: u64,
}

/// Commands that can be stored in the Raft log.
///
/// For now, this is focused on group membership changes.
/// Can be extended later for other replicated state.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum ReplicatedCommand {
	/// No-op entry, used for leader election confirmation.
	/// Leaders append this on election to commit entries from previous terms,
	/// also used as a sentinel value for log entry at index 0.
	#[default]
	Noop,

	/// Manage the replicated membership state of the group.
	Membership(MembershipCommand),
}

/// Replicated commands to modify the membership state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MembershipCommand {
	/// A new member has been added to the group or it updated its own info.
	InsertMember(Box<SignedPeerEntry>),

	/// A peer has been removed from the group.
	RemoveMember(PeerId),
}
