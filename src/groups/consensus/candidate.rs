use {
	super::protocol::RequestVote,
	crate::{
		PeerId,
		groups::{
			consensus::{ConsensusMessage, shared::Shared},
			log::{Index, StateMachine, Storage, Term},
		},
	},
	std::collections::HashSet,
};

/// Internal state for the candidate role that is currently running elections
/// for its leadership candidacy.
#[derive(Debug)]
pub struct Candidate {
	elections: Elections,
}

impl Candidate {
	/// As a candidate, we start elections and wait for votes from other nodes or
	/// `AppendEntries` from a leader. If no quorum is reached within the election
	/// timeout, we start a new election.
	pub async fn tick<S: Storage<M::Command>, M: StateMachine>(
		&mut self,
		shared: &mut Shared<S, M>,
	) {
		core::future::pending::<()>().await;
	}

	pub fn receive<S: Storage<M::Command>, M: StateMachine>(
		&mut self,
		message: ConsensusMessage,
		sender: PeerId,
		shared: &mut Shared<S, M>,
	) {
		match message {
			ConsensusMessage::RequestVote(request) => {
				// Handle incoming RequestVote messages from other candidates
				// (e.g., by comparing terms and granting or denying votes)
			}
			ConsensusMessage::RequestVoteResponse(response) => {
				// Handle incoming RequestVoteResponse messages from peers we have
				// requested votes from (e.g., by counting granted votes and checking
				// for quorum)
			}
			ConsensusMessage::AppendEntries(request) => {
				// If we receive AppendEntries from a leader with a higher term, we step
				// down to follower state
			}
			ConsensusMessage::AppendEntriesResponse(_) => {
				// Candidates ignore AppendEntriesResponse messages since they are not
				// the leader and do not expect any responses to AppendEntries messages.
			}
		}
	}
}

impl Candidate {
	pub fn term(&self) -> Term {
		0
	}
}

/// Internal state for the candidate role that is currently running elections
/// for its leadership candidacy.
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
	pub fn new() -> Self {
		todo!()
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

	/// Registers a vote granted by the specified peer and returns `true` if a
	/// quorum has been reached.
	pub fn grant(&mut self, peer_id: PeerId) -> bool {
		self.votes_granted.insert(peer_id);
		self.has_quorum()
	}
}
