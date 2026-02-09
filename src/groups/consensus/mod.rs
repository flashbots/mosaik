use {
	crate::{
		PeerId,
		groups::{
			consensus::{role::Role, shared::Shared},
			log,
			state::WorkerState,
		},
	},
	std::sync::Arc,
};

mod candidate;
mod follower;
mod leader;
mod protocol;
mod role;
mod shared;

pub(super) use protocol::ConsensusMessage;

/// The driver of the Raft consensus algorithm for a single group. This type is
/// responsible for:
///
/// - Deciding about the current role of the local node in the Raft consensus
///   algorithm (leader, follower, candidate).
///
/// - Participating in the Raft consensus algorithm according to the current
///   role.
///
/// - Exposing a public API for interacting with the application-level
///   replicated state machine that is being managed by this consensus group.
///
/// - Managing the persistent log of the group through the provided storage
///   implementation.
///
/// - Triggering new elections when the local node is not the leader and the
///   election timeout elapses without receiving heartbeats from the current
///   leader.
///
/// - Stepping down from the leader role when it receives a message from a valid
///   leader with a higher term.
///
/// - Handling incoming consensus messages from remote bonded peers in the group
///   and driving the log commitment process according to the Raft algorithm.
///
/// Notes:
///
/// - Instances of this type are owned and managed by the long-running worker
///   task that is associated with the group.
pub struct Consensus<S, M>
where
	S: log::Storage<M::Command>,
	M: log::StateMachine,
{
	/// Shared state across all raft roles.
	shared: shared::Shared<S, M>,

	/// The current role of this node in the Raft consensus algorithm and its
	/// role-specific state.
	role: role::Role,
}

impl<S, M> Consensus<S, M>
where
	S: log::Storage<M::Command>,
	M: log::StateMachine,
{
	/// Creates a new consensus instance with the given storage and state machine
	/// implementations. This is called when initializing the Worker task for a
	/// group.
	pub fn new(group: Arc<WorkerState>, storage: S, state_machine: M) -> Self {
		Self {
			shared: Shared::new(group, storage, state_machine),
			role: Role::default(),
		}
	}

	/// Accepts an incoming consensus message from a remote bonded peer in the
	/// group.
	pub fn receive(&mut self, message: ConsensusMessage, from: PeerId) {
		self.role.receive(message, from, &mut self.shared);
	}
}

// 	/// Drives the consensus state machine by ticking.
// 	/// If this node is the leader, it will send heartbeats at the configured
// 	/// interval. If it is a follower or candidate, it will wait for the
// election 	/// timeout to elapse.
// 	pub async fn tick(&mut self) {
// 		self.role.tick(&mut self.shared).await;
// 	}

// 	pub fn query(&self, query: M::Query) -> M::QueryResult {
// 		self.shared.log().query(query)
// 	}
// }

// /// Returns a random election timeout duration.
// fn random_election_timeout(config: &IntervalsConfig) -> Duration {
// 	let base = config.election_timeout;
// 	let jitter = config.election_timeout_jitter;
// 	let range_start = base;
// 	let range_end = base + jitter;
// 	random_range(range_start..range_end)
// }
