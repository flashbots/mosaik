use {
	crate::{
		Consistency,
		PeerId,
		groups::{
			CommandError,
			Index,
			QueryError,
			log,
			raft::{role::Role, shared::Shared},
			state::WorkerState,
		},
		primitives::{BoxPinFut, InternalFutureExt, Short},
	},
	core::{
		future::ready,
		pin::Pin,
		task::{Context, Poll},
	},
	futures::Stream,
	std::sync::Arc,
};

mod candidate;
mod follower;
mod leader;
mod protocol;
mod role;
mod shared;
mod sync;

pub(super) use protocol::Message;
use {
	bincode::{config::standard, serde::decode_from_std_read},
	bytes::{Buf, Bytes},
	futures::FutureExt,
};

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
	S: log::Storage<M::Command>,
	M: log::StateMachine,
{
	/// The current role of this node in the Raft consensus algorithm and its
	/// role-specific state.
	role: role::Role<M>,

	/// Shared state across all raft roles.
	shared: shared::Shared<S, M>,
}

impl<S, M> Raft<S, M>
where
	S: log::Storage<M::Command>,
	M: log::StateMachine,
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
		let Ok(message) = decode_from_std_read(&mut buffer.reader(), standard())
		else {
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

	/// Appends the command to the current leader's log and returns a future that
	/// resolves when the command is replicated and committed to the state.
	pub fn execute(
		&mut self,
		command: M::Command,
	) -> BoxPinFut<Result<Index, CommandError<M>>> {
		match &mut self.role {
			Role::Leader(leader) => {
				// add the command to the leader's log
				let expected_index = leader.enqueue_command(command, &self.shared);

				// return a future that resolves when the command is committed
				let fut = self.shared.when().committed_up_to(expected_index);
				fut.map(move |_| Ok(expected_index)).pin()
			}

			Role::Follower(_follower) => todo!("execute as follower"),

			// nodes in candidate state cannot accept commands
			Role::Candidate(_) => ready(Err(CommandError::Offline(command))).pin(),
		}
	}

	/// Sends the command to the current leader without waiting for it to be
	/// committed to the state machine. The returned future will resolve once the
	/// command has been sent to the leader.
	pub fn feed(
		&mut self,
		command: M::Command,
	) -> impl Future<Output = Result<(), CommandError<M>>> + Send + Sync + 'static
	{
		match &mut self.role {
			Role::Leader(leader) => {
				leader.enqueue_command(command, &self.shared);
				ready(Ok(()))
			}
			Role::Follower(_follower) => todo!("feed as follower"),
			Role::Candidate(_) => ready(Err(CommandError::Offline(command))),
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
		&self,
		query: M::Query,
		consistency: Consistency,
	) -> BoxPinFut<Result<M::QueryResult, QueryError<M>>> {
		match &self.role {
			Role::Leader(_) => {
				// if the local node is the leader, it can process the query directly
				// against the state machine, which is always up to date with the latest
				// committed state of the group.
				ready(Ok(self.shared.log.query(query))).pin()
			}

			Role::Follower(_) => match consistency {
				Consistency::Weak => {
					// with weak consistency, the follower can process the query against
					// its local state machine up to the last known committed state.
					ready(Ok(self.shared.log.query(query))).pin()
				}
				Consistency::Strong => todo!("strong query as follower"),
			},

			Role::Candidate(_) => {
				// if the local node is a candidate, it cannot guarantee that its state
				// is up to date with the latest committed state of the group, so it
				// should reject the query with an appropriate error.
				ready(Err(QueryError::Offline(query))).pin()
			}
		}
	}
}

/// This stream is polled by the group worker loop and is responsible for
/// driving the raft consensus protocol. Depending on the currently assumed role
/// of the local node in the consensus algorithm, it will trigger different
/// periodic actions, such as starting new elections when the node is a follower
/// or sending heartbeats when the node is a leader.
impl<S, M> Stream for Raft<S, M>
where
	S: log::Storage<M::Command>,
	M: log::StateMachine,
{
	type Item = ();

	fn poll_next(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Self::Item>> {
		let this = self.get_mut();
		let shared = &mut this.shared;
		this.role.poll_next_tick(cx, shared)
	}
}
