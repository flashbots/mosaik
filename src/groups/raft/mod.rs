use {
	crate::{
		PeerId,
		groups::{
			log,
			raft::{role::Role, shared::Shared},
			state::WorkerState,
		},
		primitives::Short,
	},
	core::{
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

pub(super) use protocol::Message;
use {
	bincode::{config::standard, serde::decode_from_std_read},
	bytes::{Buf, Bytes},
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
	role: role::Role<S, M>,

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

		Self {
			role: Role::new(&shared),
			shared,
		}
	}

	/// Accepts an incoming consensus message from a remote bonded peer in the
	/// group and decode it into a strongly-typed `Message` that is aware of the
	/// state machine implementation used by the group.
	pub fn receive(&mut self, buffer: Bytes, from: PeerId) {
		let Ok(message) = decode_from_std_read(&mut buffer.reader(), standard())
		else {
			tracing::warn!(
				peer = %Short(from),
				group = %Short(self.shared.group().group_id()),
				network = %Short(self.shared.group().network_id()),
				"failed to decode incoming raft message",
			);
			return;
		};

		tracing::trace!(
			message = ?message,
			group = %Short(self.shared.group().group_id()),
			peer = %Short(from),
			network = %Short(self.shared.group().network_id()),
			"received",
		);

		self.role.receive(message, from, &mut self.shared);
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
