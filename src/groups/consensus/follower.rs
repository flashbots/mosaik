use crate::{
	PeerId,
	groups::{
		consensus::{ConsensusMessage, shared::Shared},
		log::Term,
	},
};

#[derive(Debug)]
pub struct Follower {
	term: Term,
	leader: Option<PeerId>,
}

impl Follower {
	pub fn new(term: Term, leader: Option<PeerId>) -> Self {
		Follower { term, leader }
	}
}

impl Follower {
	/// As a follower, we wait for messages from leaders or candidates.
	/// If no messages are received within the election timeout, we transition to
	/// a candidate state and start a new election.
	pub async fn tick(&mut self, shared: &mut Shared) {
		core::future::pending::<()>().await;
	}

	pub fn receive(
		&mut self,
		message: ConsensusMessage,
		sender: PeerId,
		shared: &mut Shared,
	) {
		match message {
			ConsensusMessage::RequestVote(request) => {}
			ConsensusMessage::RequestVoteResponse(response) => {}
			ConsensusMessage::AppendEntries(request) => {}
			ConsensusMessage::AppendEntriesResponse(_) => {
				// Followers ignore `AppendEntriesResponse` messages since they are not
				// the leader and do not expect any responses to `AppendEntries`
				// messages.
			}
		}
	}
}

impl Follower {
	pub const fn term(&self) -> Term {
		self.term
	}

	pub const fn leader(&self) -> Option<PeerId> {
		self.leader
	}
}
