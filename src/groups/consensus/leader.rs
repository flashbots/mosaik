use crate::{
	PeerId,
	groups::{
		consensus::{ConsensusMessage, shared::Shared},
		log::{StateMachine, Storage, Term},
	},
};

#[derive(Debug)]
pub struct Leader;

impl Leader {
	/// As a leader, we send `AppendEntries` with new log entries or as heartbeats
	/// to all followers. We also handle client requests for log mutations.
	///
	/// If we receive an `AppendEntries` from another leader with a higher term,
	/// we step down to follower state and follow that leader.
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
			ConsensusMessage::RequestVote(request) => {}
			ConsensusMessage::RequestVoteResponse(response) => {}
			ConsensusMessage::AppendEntries(_) => {}
			ConsensusMessage::AppendEntriesResponse(_) => {}
		}
	}
}

impl Leader {
	pub fn term(&self) -> Term {
		0
	}
}
