use {
	crate::{
		PeerId,
		groups::{
			Command,
			log::{Index, Term},
		},
		primitives::Short,
	},
	core::ops::RangeInclusive,
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

	/// Sent by a lagging follower to discover which peers have the log entries
	/// it is missing and to coordinate the catch-up process.
	LogSyncDiscovery(LogSyncDiscovery),

	/// Sent to individual peers to request the log entries in the specified
	/// range during the catch-up process.
	LogSyncRequest(LogSyncRequest),

	/// Response to a `LogSyncRequest` containing the requested historical log
	/// entries.
	LogSyncResponse(LogSyncResponse<C>),
}

impl<C: Command> Message<C> {
	/// Returns the term carried by the message.
	pub const fn term(&self) -> Option<Term> {
		match self {
			Self::AppendEntries(msg) => Some(msg.term),
			Self::AppendEntriesResponse(msg) => Some(msg.term),
			Self::RequestVote(msg) => Some(msg.term),
			Self::RequestVoteResponse(msg) => Some(msg.term),
			Self::LogSyncDiscovery(_)
			| Self::LogSyncRequest(_)
			| Self::LogSyncResponse(_) => None,
		}
	}

	/// If the message was sent by a leader, returns its peer ID.
	pub const fn leader(&self) -> Option<PeerId> {
		match self {
			Self::AppendEntries(msg) => Some(msg.leader),
			Self::AppendEntriesResponse(_)
			| Self::RequestVote(_)
			| Self::RequestVoteResponse(_)
			| Self::LogSyncDiscovery(_)
			| Self::LogSyncRequest(_)
			| Self::LogSyncResponse(_) => None,
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

#[derive(Debug, Clone, Display, Serialize, Deserialize)]
pub enum Vote {
	/// Vote granted to the candidate or the leader by a voting follower that is
	/// in sync with the log.
	Granted,

	/// Vote denied to the candidate during elections.
	Denied,

	/// Abstain from voting because the follower is lagging behind the leader or
	/// candidate's log progress. This will remove the node from the quorum
	/// denominator until it catches up with the log, but will still allow it to
	/// receive log entries and become a voting member again once it is back in
	/// sync.
	Abstained,
}

/// `RequestVote` Message response.
#[derive(Debug, Clone, Display, Serialize, Deserialize)]
#[display("{vote}@{term}")]
pub struct RequestVoteResponse {
	/// Current term, for candidate to update itself.
	pub term: Term,

	/// The vote granted to the candidate.
	pub vote: Vote,
}

/// Log entry stored in the Raft log.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound(deserialize = "C: DeserializeOwned"))]
pub struct LogEntry<C: Command> {
	/// Term when entry was received by leader.
	pub term: Term,

	/// Short-lived random nonce to correlate commands forwarded to the leader
	/// with the log entries created by those commands. This is used to confirm
	/// that a command has been processed and replicated by the leader.
	pub nonce: u64,

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

	/// The index of the last log entry the follower has after processing this
	/// `AppendEntries`. Used by the leader to determine which entries have
	/// been replicated to a majority and can be committed. Only meaningful
	/// when `vote` is `Granted`.
	pub last_log_index: Index,

	/// Granted if follower contained entry matching `prev_log_index` and
	/// `prev_log_term`. Abstain if the follower is lagging behind and cannot
	/// verify the log consistency, which will exclude it from the quorum until
	/// it catches up.
	pub vote: Vote,
}

/// Message broadcasted by a lagging follower to all bonded peers to discover
/// which peers have the log entries it is missing and to coordinate the
/// catch-up process.
#[derive(Debug, Clone, Display, Serialize, Deserialize)]
#[display("[{}..{}]", _0.start(), _0.end())]
pub struct LogSyncDiscovery(RangeInclusive<Index>);

/// Message sent to individual peers to request the log entries in the specified
/// range during the catch-up process.
#[derive(Debug, Clone, Display, Serialize, Deserialize)]
#[display("[{}..{}]", _0.start(), _0.end())]
pub struct LogSyncRequest(RangeInclusive<Index>);

/// Response to a `LogSyncRequest` containing the requested historical log
/// entries.
#[derive(Debug, Clone, Display, Serialize, Deserialize)]
#[serde(bound(deserialize = "C: DeserializeOwned"))]
#[display("{}[{}..{}]", range.start(), range.end(), entries.len())]
pub struct LogSyncResponse<C: Command> {
	pub range: RangeInclusive<Index>,
	pub entries: Vec<LogEntry<C>>,
}
