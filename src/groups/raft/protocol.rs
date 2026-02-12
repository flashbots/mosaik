use {
	crate::{
		PeerId,
		groups::{
			Command,
			Cursor,
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

	/// Sent by followers to the leader to forward client commands.
	ForwardCommands(ForwardCommands<C>),

	/// Response sent by the leader to followers that forwarded client commands
	/// with a `request_id` to inform them of the log index assigned to the
	/// command.
	ForwardCommandResponse(ForwardCommandResponse),

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
			Self::ForwardCommands(_)
			| Self::ForwardCommandResponse(_)
			| Self::LogSyncDiscovery(_)
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
			| Self::ForwardCommands(_)
			| Self::ForwardCommandResponse(_)
			| Self::LogSyncDiscovery(_)
			| Self::LogSyncRequest(_)
			| Self::LogSyncResponse(_) => None,
		}
	}
}

/// `RequestVote` Message arguments.
#[derive(Debug, Clone, Display, Serialize, Deserialize)]
#[display("{}[t{term}/log={log_position}]", Short(candidate))]
pub struct RequestVote {
	/// Candidate's term.
	pub term: Term,

	/// Candidate requesting vote.
	pub candidate: PeerId,

	/// Term and index of candidate's last log entry.
	pub log_position: Cursor,
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

	/// Term and Index of log entry immediately preceding new ones.
	pub prev_log_position: Cursor,

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

/// Message sent by followers to the leader to forward client commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound(deserialize = "C: DeserializeOwned"))]
pub struct ForwardCommands<C: Command> {
	/// The command to be executed by the leader and replicated to the group.
	pub commands: Vec<C>,

	/// Optionally when set, the leader will respond back to the sender with a
	/// `ForwardCommandResponse` containing the log index assigned to the
	/// appended command so that the follower can track the commit progress of
	/// the log and determine when the command has been committed to the state
	/// machine.
	///
	/// This value is set when calling `Group::execute` from a follower, and is
	/// `None` when calling `Group::feed` from a follower since `feed` does not
	/// wait for the command to be committed and thus does not need to track the
	/// commit progress of the command.
	///
	/// This value is randomly generated by the follower.
	pub request_id: Option<u64>,
}

/// Response sent by the leader to followers that forwarded client commands with
/// a `request_id` to inform them of the log index assigned to the appended
/// command so that the followers can track the commit progress of the log and
/// determine when the command has been committed to the state machine.
#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
pub struct ForwardCommandResponse {
	/// The `request_id` from the original `ForwardCommand` sent by the follower
	pub request_id: u64,

	/// The log index assigned to the appended command by the leader, which the
	/// follower can use to track the commit progress of the log and determine
	/// when the command has been committed to the state machine.
	pub log_index: Index,
}
