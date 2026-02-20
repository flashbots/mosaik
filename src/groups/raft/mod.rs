use {
	crate::{
		Consistency,
		PeerId,
		groups::{
			CommandError,
			CommittedQueryResult,
			IndexRange,
			QueryError,
			StateMachine,
			StateSyncContext,
			Storage,
			raft::{role::Role, shared::Shared},
			state::WorkerState,
		},
		primitives::{BoxPinFut, InternalFutureExt, Short, deserialize},
	},
	bytes::Bytes,
	core::{
		future::ready,
		task::{Context, Poll},
	},
	std::sync::Arc,
};

mod candidate;
mod follower;
mod leader;
mod protocol;
mod role;
mod shared;

pub(super) use protocol::Message;

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
pub struct Raft<S, M>
where
	S: Storage<M::Command>,
	M: StateMachine,
{
	/// The current role of this node in the Raft consensus algorithm and its
	/// role-specific state.
	role: role::Role<M>,

	/// Shared state across all raft roles.
	shared: shared::Shared<S, M>,
}

impl<S, M> Raft<S, M>
where
	S: Storage<M::Command>,
	M: StateMachine,
{
	/// Creates a new consensus instance with the given storage and state machine
	/// implementations. This is called when initializing the Worker task for a
	/// group.
	pub fn new(group: Arc<WorkerState>, storage: S, state_machine: M) -> Self {
		let shared = Shared::new(group, storage, state_machine);
		let role = Role::new(&shared);
		Self { role, shared }
	}

	/// Accepts an incoming consensus message from a remote bonded peer in the
	/// group and decode it into a strongly-typed `Message` that is aware of the
	/// state machine implementation used by the group.
	pub fn receive_protocol_message(&mut self, buffer: Bytes, from: PeerId) {
		let Ok(message) = deserialize(buffer) else {
			tracing::warn!(
				peer = %Short(from),
				group = %Short(self.shared.group_id()),
				network = %Short(self.shared.network_id()),
				"failed to decode incoming raft message",
			);
			return;
		};

		self
			.role
			.receive_protocol_message(message, from, &mut self.shared);
	}

	/// Sends the command to the current leader without waiting for it to be
	/// committed to the state machine. The returned future will resolve once the
	/// command has been assigned an index by the leader, but it does not
	/// guarantee that the command has been committed to the state.
	pub fn feed(
		&mut self,
		commands: Vec<M::Command>,
	) -> BoxPinFut<Result<IndexRange, CommandError<M>>> {
		match &mut self.role {
			Role::Leader(leader) => {
				// add the command to the leader's log and return immediately with the
				// assigned index, without waiting for it to be committed
				ready(Ok(leader.enqueue_commands(commands, &self.shared))).pin()
			}

			// followers forward the command to the current leader and return a future
			// that resolves when the command is acknowledged and assigned an index.
			Role::Follower(follower) => {
				follower.forward_commands(commands, &self.shared).pin()
			}
			Role::Candidate(_) => {
				// nodes in candidate state cannot accept or forward commands
				ready(Err(CommandError::Offline(commands))).pin()
			}
		}
	}

	/// Queries the current committed state of the group's state machine.
	///
	/// Depending on the consistency level, the query may be processed by the
	/// local node without any guarantee of consistency with the current state of
	/// the state machine (e.g. if the local node is a follower that is not up to
	/// date with the leader), or it may require the local node forward the query
	/// to the current leader and wait for a response that reflects the latest
	/// committed state of the state machine.
	pub fn query(
		&mut self,
		query: M::Query,
		consistency: Consistency,
	) -> BoxPinFut<Result<CommittedQueryResult<M>, QueryError<M>>> {
		match &mut self.role {
			Role::Leader(_) => {
				// if the local node is the leader, it can process the query directly
				// against the state machine, which is always up to date with the latest
				// committed state of the group.
				ready(Ok(CommittedQueryResult {
					result: self.shared.machine().query(query),
					at_position: self.shared.committed(),
				}))
				.pin()
			}

			Role::Follower(follower) => match consistency {
				Consistency::Weak => {
					// with weak consistency, the follower can process the query against
					// its local state machine up to the last locally known committed
					// state.
					ready(Ok(CommittedQueryResult {
						result: self.shared.machine().query(query),
						at_position: self.shared.committed(),
					}))
					.pin()
				}
				Consistency::Strong => {
					// With strong consistency, only the leader can guarantee that the
					// query was processed against the latest committed state of the
					// group, so the follower must forward the query to the leader and
					// wait for a response.
					follower.forward_query(query, &self.shared)
				}
			},

			Role::Candidate(_) => {
				// if the local node is a candidate, it cannot guarantee that its state
				// is up to date with the latest committed state of the group, so it
				// should reject the query with an appropriate error.
				ready(Err(QueryError::Offline(query))).pin()
			}
		}
	}

	/// Polled by the group worker loop and is responsible for driving the raft
	/// consensus protocol. Depending on the currently assumed role of the local
	/// node  in the consensus algorithm, it will trigger different periodic
	/// actions, such as starting new elections when the node is a follower or
	/// sending heartbeats when the node is a leader.
	pub fn poll_next_tick(&mut self, cx: &mut Context<'_>) -> Poll<()> {
		self.role.poll_next_tick(cx, &mut self.shared)
	}
}
