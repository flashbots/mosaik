use {
	crate::{
		PeerId,
		discovery::SignedPeerEntry,
		groups::{
			Bond,
			Config,
			group::GroupState,
			log::{Index, Log, LogEntry, Term},
			wire::BondMessage,
		},
		primitives::Short,
	},
	core::{iter::once, time::Duration},
	derive_more::Display,
	rand::random_range,
	serde::{Deserialize, Serialize},
	std::{collections::HashSet, sync::Arc, time::Instant},
	tokio::time::sleep_until,
};

/// Raft node state - each node is always in one of these states.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Role {
	/// Passive state: responds to messages from candidates and leaders.
	/// If election timeout elapses without receiving `AppendEntries` from
	/// current leader or granting vote to candidate, converts to candidate.
	Follower { term: Term, leader: Option<PeerId> },

	/// Active state during elections: increments term, votes for self,
	/// sends `RequestVote` RPCs to all other servers, and waits for votes.
	Candidate { elections: Elections },

	/// Active state: handles all client requests, sends `AppendEntries` RPCs
	/// to followers, and sends periodic heartbeats.
	Leader { term: Term },
}

impl Role {
	pub fn term(&self) -> Term {
		match self {
			Role::Candidate { elections, .. } => elections.term,
			Role::Leader { term } | Role::Follower { term, .. } => *term,
		}
	}

	pub fn is_leader(&self) -> bool {
		matches!(self, Role::Leader { .. })
	}

	pub fn is_follower(&self) -> bool {
		matches!(self, Role::Follower { .. })
	}

	pub fn is_candidate(&self) -> bool {
		matches!(self, Role::Candidate { .. })
	}
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Elections {
	/// The current term for this node.
	term: Term,

	/// The peer ID of the candidate requesting votes.
	candidate: PeerId,

	/// The set of peers from which votes have been requested.
	requested_from: HashSet<PeerId>,

	/// The set of peers that have granted their vote.
	votes_granted: HashSet<PeerId>,

	/// The index of the last log entry for log comparison.
	last_log_index: Index,

	/// The term of the last log entry for log comparison.
	last_log_term: Term,
}

impl Elections {
	pub fn new(current: &Consensus) -> Self {
		let local_id = current.group.local.id();
		Self {
			term: current.role.term() + 1,
			candidate: local_id,
			requested_from: current
				.group
				.bonds
				.iter()
				.map(|b| *b.peer().id())
				.chain(once(local_id))
				.collect(),
			votes_granted: HashSet::from([local_id]),
			last_log_index: current.log.index(),
			last_log_term: current.log.term(),
		}
	}

	pub fn request(&self) -> RequestVote {
		RequestVote {
			term: self.term,
			candidate: self.candidate,
			last_log_index: self.last_log_index,
			last_log_term: self.last_log_term,
		}
	}

	pub fn has_quorum(&self) -> bool {
		let total_nodes = self.requested_from.len();
		let votes = self.votes_granted.len();
		let quorum = (total_nodes / 2) + 1;
		votes >= quorum
	}
}

pub struct Consensus {
	group: Arc<GroupState>,

	/// The current role of this node in the Raft consensus algorithm.
	role: Role,

	/// The candidate that received vote in the current term, if any.
	voted_for: Option<(Term, PeerId)>,

	/// The persistent log for this group that tracks all changes to the group's
	/// replicated state machine through raft.
	log: Log<ReplicatedCommand>,

	/// Fires when the election timeout elapses without hearing from a leader,
	/// this will trigger new elections.
	next_election_at: Instant,

	/// The last time we have sent an `AppendEntries` (or noop heartbeat) message
	/// to all followers.
	last_append_at: Instant,
}

impl Consensus {
	pub fn new(group: Arc<GroupState>) -> Self {
		let config = &group.config;
		let first_election_after =
			random_election_timeout(config) + config.bootstrap_delay;

		let log = Log::new();
		let term = log.term();

		Self {
			group,
			log,
			voted_for: None,
			last_append_at: Instant::now(),
			next_election_at: Instant::now() + first_election_after,
			role: Role::Follower { term, leader: None },
		}
	}

	/// Returns `true` if this node is currently the leader of the group.
	pub fn is_leader(&self) -> bool {
		self.role.is_leader()
	}

	/// Returns the current leader of the group, if known.
	pub fn leader(&self) -> Option<PeerId> {
		match &self.role {
			Role::Follower { leader, .. } => *leader,
			Role::Leader { .. } => Some(self.group.local.id()),
			Role::Candidate { .. } => None,
		}
	}

	/// Drives the consensus state machine by ticking.
	/// If this node is the leader, it will send heartbeats at the configured
	/// interval. If it is a follower or candidate, it will wait for the election
	/// timeout to elapse.
	pub async fn tick(&mut self) {
		if self.is_leader() {
			let next_append_in = self
				.group
				.config
				.heartbeat_interval
				.saturating_sub(self.last_append_at.elapsed());

			sleep_until((Instant::now() + next_append_in).into()).await;
			self.send_heartbeat();
		} else {
			sleep_until(self.next_election_at.into()).await;
			self.maybe_start_elections();
		}
	}

	/// Resolves when the election timeout elapses without hearing from a leader.
	/// This will put the current node into a candidate state and start a new
	/// election by sending `RequestVote` messages to all nodes that we have bonds
	/// to.
	pub fn maybe_start_elections(&mut self) {
		if self.role.is_leader() {
			return;
		}

		self.next_election_at =
			Instant::now() + random_election_timeout(&self.group.config);

		let elections = Elections::new(self);

		// If we already have a quorum of votes (e.g. single-node cluster),
		// we can immediately become the leader without sending `RequestVote`.
		if elections.has_quorum() {
			tracing::info!(
				group = %Short(self.group.key.id()),
				network = %self.group.local.network_id(),
				term = elections.term,
				"elected self as leader by quorum without sending RequestVote",
			);

			self.role = Role::Leader {
				term: elections.term,
			};
			return;
		}

		self.role = Role::Candidate {
			elections: elections.clone(),
		};

		let request = elections.request();

		// by becoming a candidate we implicitly vote for ourself
		self.voted_for = Some((elections.term, elections.candidate));

		tracing::info!(
			group = %Short(self.group.key.id()),
			network = %self.group.local.network_id(),
			term = request.term,
			candidate = %Short(request.candidate),
			"starting new group leader elections",
		);

		let message = ConsensusMessage::RequestVote(request);
		let message = BondMessage::Consensus(message);

		for bond in self.group.bonds.iter() {
			bond.send(message.clone());
		}
	}

	/// Accepts an incoming consensus message from another peer in the group.
	pub fn accept_message(&mut self, message: ConsensusMessage, from: PeerId) {
		// Check for higher term in ANY incoming message
		let incoming_term = match &message {
			ConsensusMessage::RequestVote(r) => r.term,
			ConsensusMessage::RequestVoteResponse(r) => r.term,
			ConsensusMessage::AppendEntries(r) => r.term,
			ConsensusMessage::AppendEntriesResponse(r) => r.term,
		};

		self.maybe_step_down(incoming_term);

		match message {
			ConsensusMessage::RequestVote(request) => {
				self.on_request_vote(request, from);
			}
			ConsensusMessage::RequestVoteResponse(response) => {
				self.on_request_vote_response(response, from);
			}
			ConsensusMessage::AppendEntries(request) => {
				self.on_append_entries(request, from);
			}
			ConsensusMessage::AppendEntriesResponse(response) => {
				self.on_append_entries_response(response, from);
			}
		}
	}

	/// If we discover a higher term, immediately step down to follower.
	/// This is the key mechanism that resolves split-brain scenarios.
	fn maybe_step_down(&mut self, incoming_term: Term) -> bool {
		let current_term = self.role.term();

		if incoming_term > current_term {
			tracing::info!(
					group = %Short(self.group.key.id()),
					network = %self.group.local.network_id(),
					current_term = current_term,
					incoming_term = incoming_term,
					"discovered higher term, stepping down to follower",
			);

			self.role = Role::Follower {
				term: incoming_term,
				leader: None,
			};

			self.voted_for = None; // Reset vote for the new term
			return true; // We stepped down
		}
		false
	}

	/// Handles an incoming `RequestVote` message from another peer.
	///
	/// Peers grand their vote to requests if:
	/// - The candidate's term is at least as large as the current term.
	/// - The candidate's log is at least as up-to-date as the local log.
	/// - The peer hasn't already voted for another candidate in the current term.
	fn on_request_vote(&mut self, request: RequestVote, from: PeerId) {
		tracing::info!(
			network = %self.group.local.network_id(),
			group = %Short(self.group.key.id()),
			request = %request,
			"raft RequestVote received",
		);

		let deny_vote = |bond: &Bond| {
			let response = RequestVoteResponse {
				term: self.role.term(),
				vote_granted: false,
			};
			let message = ConsensusMessage::RequestVoteResponse(response);
			let message = BondMessage::Consensus(message);
			bond.send(message);
		};

		let Some(bond) = self.group.bonds.get(&from) else {
			tracing::debug!(
				network = %self.group.local.network_id(),
				group = %Short(self.group.key.id()),
				from = %Short(from),
				"received RequestVote from disconnected peer - ignoring",
			);
			return;
		};

		if request.term < self.role.term() {
			deny_vote(&bond);
			return;
		}

		if let Some((voted_term, peer_id)) = &self.voted_for {
			if *voted_term >= request.term && *peer_id != request.candidate {
				deny_vote(&bond);
				return;
			}
		}

		if request.last_log_index < self.log.index()
			|| request.last_log_term < self.log.term()
		{
			deny_vote(&bond);
			return;
		}

		self.voted_for = Some((request.term, request.candidate));
		let response = RequestVoteResponse {
			term: request.term,
			vote_granted: true,
		};
		let message = ConsensusMessage::RequestVoteResponse(response);
		let message = BondMessage::Consensus(message);
		bond.send(message);
	}

	/// Handles an incoming `RequestVoteResponse` message from another peer
	/// that was sent in response to our own `RequestVote` message.
	///
	/// Ignores the response if we are not currently a candidate.
	fn on_request_vote_response(
		&mut self,
		response: RequestVoteResponse,
		from: PeerId,
	) {
		tracing::trace!(
			network = %self.group.local.network_id(),
			group = %Short(self.group.key.id()),
			response = %response,
			from = %Short(from),
			"raft RequestVoteResponse received",
		);

		let Role::Candidate { ref mut elections } = self.role else {
			tracing::debug!(
				network = %self.group.local.network_id(),
				group = %Short(self.group.key.id()),
				"received RequestVoteResponse while not a candidate - ignoring",
			);
			return;
		};

		if elections.term != response.term {
			tracing::debug!(
				network = %self.group.local.network_id(),
				group = %Short(self.group.key.id()),
				"received RequestVoteResponse for different term - ignoring",
			);
			return;
		}

		elections.votes_granted.insert(from);

		if elections.has_quorum() {
			tracing::info!(
				group = %Short(self.group.key.id()),
				network = %self.group.local.network_id(),
				term = elections.term,
				"elected self as leader by receiving quorum of votes [{}/{}]",
				elections.votes_granted.len(),
				elections.requested_from.len(),
			);

			let term = elections.term;
			self.assume_leadership(term);
		}
	}

	fn on_append_entries(
		&mut self,
		request: AppendEntries<ReplicatedCommand>,
		from: PeerId,
	) {
		tracing::trace!(
			network = %self.group.local.network_id(),
			group = %Short(self.group.key.id()),
			request = ?request,
			from = %Short(from),
			"received AppendEntries raft request",
		);

		if self.role.term() < request.term {
			self.role = Role::Follower {
				term: request.term,
				leader: Some(request.leader),
			};

			tracing::info!(
				network = %self.group.local.network_id(),
				group = %Short(self.group.key.id()),
				term = request.term,
				leader = %Short(request.leader),
				"stepping down to follower for new leader",
			);
		}

		self.next_election_at =
			Instant::now() + random_election_timeout(&self.group.config);
	}

	fn on_append_entries_response(
		&mut self,
		response: AppendEntriesResponse,
		from: PeerId,
	) {
		tracing::trace!(
			network = %self.group.local.network_id(),
			group = %Short(self.group.key.id()),
			response = ?response,
			from = %Short(from),
			"received AppendEntriesResponse raft response",
		);
	}

	/// Transitions this node into the leader role for the specified term.
	fn assume_leadership(&mut self, term: Term) {
		if self.role.term() > term {
			return;
		}

		self.role = Role::Leader { term };
		self.send_heartbeat();
	}

	/// Sends heartbeat `AppendEntries` (empty entries) to maintain leadership.
	/// Should be called periodically (e.g., every `heartbeat_interval`).
	pub fn send_heartbeat(&mut self) {
		let Role::Leader { term } = self.role else {
			return;
		};

		let leader = self.group.local.id();

		let request = AppendEntries {
			term,
			leader,
			prev_log_index: self.log.index(),
			prev_log_term: self.log.term(),
			entries: vec![], // Empty = heartbeat
			leader_commit: self.log.committed(),
		};

		let message = ConsensusMessage::AppendEntries(request);
		let message = BondMessage::Consensus(message);

		for bond in self.group.bonds.iter() {
			bond.send(message.clone());
		}

		self.last_append_at = Instant::now();
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) enum ConsensusMessage {
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

/// Returns a random election timeout duration.
fn random_election_timeout(config: &Config) -> Duration {
	let base = config.election_timeout;
	let jitter = config.election_timeout_jitter;
	let range_start = base;
	let range_end = base + jitter;
	random_range(range_start..range_end)
}
