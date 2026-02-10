use {
	crate::{
		PeerId,
		groups::{
			Command,
			log::{Index, Term},
		},
		primitives::Short,
	},
	derive_more::{Display, From},
	serde::{Deserialize, Serialize, de::DeserializeOwned},
};

/// Raft messages as defined in the Raft consensus algorithm.
#[derive(Debug, Clone, Serialize, Deserialize, From)]
#[serde(bound(deserialize = "C: DeserializeOwned"))]
pub enum Message<C: Command> {
	/// Sent by leaders to assert authority (heartbeat) and replicate log
	/// entries. When `entries` is empty, this is a pure heartbeat.
	AppendEntries(AppendEntries<C>),

	/// Response to an `AppendEntries` message.
	AppendEntriesResponse(AppendEntriesResponse),

	/// Sent by candidates to gather votes during an election.
	RequestVote(RequestVote),

	/// Response to a `RequestVote` message.
	RequestVoteResponse(RequestVoteResponse),
}

impl<C: Command> Message<C> {
	/// Returns the term carried by the message.
	/// All raft messages include the sender's current term.
	pub const fn term(&self) -> Term {
		match self {
			Self::AppendEntries(msg) => msg.term,
			Self::AppendEntriesResponse(msg) => msg.term,
			Self::RequestVote(msg) => msg.term,
			Self::RequestVoteResponse(msg) => msg.term,
		}
	}

	/// If the message was sent by a leader, returns its peer ID.
	pub const fn leader(&self) -> Option<PeerId> {
		match self {
			Self::AppendEntries(msg) => Some(msg.leader),
			Self::AppendEntriesResponse(_)
			| Self::RequestVote(_)
			| Self::RequestVoteResponse(_) => None,
		}
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

/// Log entry stored in the Raft log.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound(deserialize = "C: DeserializeOwned"))]
pub struct LogEntry<C: Command> {
	/// Term when entry was received by leader.
	pub term: Term,

	/// Command for replicated state machine. This is the application-specific
	/// state transition that is replicated across the group via the Raft log.
	pub command: C,
}

/// `AppendEntries` message arguments.
///
/// Sent by leader to replicate log entries and as heartbeat.
/// When `entries` is empty, this serves as a heartbeat to maintain leadership.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound(deserialize = "C: DeserializeOwned"))]
pub struct AppendEntries<C: Command> {
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
