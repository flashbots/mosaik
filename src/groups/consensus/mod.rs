use {
	crate::{
		PeerId,
		groups::{
			Bond,
			Config,
			consensus::{role::Role, shared::Shared},
			group::GroupState,
		},
	},
	core::time::Duration,
	rand::random_range,
	std::sync::Arc,
};

mod candidate;
mod follower;
mod leader;
mod protocol;
mod role;
mod shared;

pub(super) use protocol::ConsensusMessage;

pub struct Consensus {
	/// Shared state across all raft roles.
	shared: shared::Shared,

	/// The current role of this node in the Raft consensus algorithm and its
	/// role-specific state.
	role: role::Role,
}

impl Consensus {
	pub fn new(group: Arc<GroupState>) -> Self {
		Self {
			shared: Shared::new(group),
			role: Role::default(),
		}
	}

	/// Drives the consensus state machine by ticking.
	/// If this node is the leader, it will send heartbeats at the configured
	/// interval. If it is a follower or candidate, it will wait for the election
	/// timeout to elapse.
	pub async fn tick(&mut self) {
		self.role.tick(&mut self.shared).await;
	}

	/// Accepts an incoming consensus message from a remote bonded peer in the
	/// group.
	pub fn receive(&mut self, message: ConsensusMessage, from: PeerId) {
		self.role.receive(message, from, &mut self.shared);
	}
}

/// Returns a random election timeout duration.
fn random_election_timeout(config: &Config) -> Duration {
	let base = config.election_timeout;
	let jitter = config.election_timeout_jitter;
	let range_start = base;
	let range_end = base + jitter;
	random_range(range_start..range_end)
}
