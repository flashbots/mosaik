use {
	crate::{
		PeerId,
		groups::{
			log::{StateMachine, Storage, Term},
			raft::{Message, protocol::AppendEntries, role::Role, shared::Shared},
		},
		primitives::Short,
	},
	core::{
		marker::PhantomData,
		ops::ControlFlow,
		pin::Pin,
		task::{Context, Poll},
		time::Duration,
	},
	std::time::Instant,
	tokio::time::{Sleep, sleep},
};

/// In the leader role, the node is active and handles log-mutating requests
/// from clients, replicates log entries, and sends periodic heartbeats to
/// followers if no log entries are being replicated within the configured
/// heartbeat interval. If the leader receives an `AppendEntries` message from
/// another leader with a higher term, it steps down to follower state and
/// follows that leader.
#[derive(Debug)]
pub struct Leader<S: Storage<M::Command>, M: StateMachine> {
	/// The current term for this node.
	term: Term,

	/// The interval at which the leader sends heartbeats to followers if no log
	/// entries are being replicated.
	heartbeat_interval: Duration,

	/// Fires at the configured interval to trigger sending empty `AppendEntries`
	/// heartbeats to followers if no log entries are being replicated within
	/// the heartbeat interval.
	heartbeat_timeout: Pin<Box<Sleep>>,

	/// Pending client commands that have not yet been replicated to followers.
	/// These commands will be included in the next `AppendEntries` message sent
	/// to followers.
	#[expect(dead_code)]
	pending_commands: Vec<M::Command>,

	/// Wakers for tasks that are waiting for the leader to either send
	/// heartbeats or replicate log entries.
	wakers: Vec<std::task::Waker>,

	#[doc(hidden)]
	_phantom: PhantomData<(S, M)>,
}

impl<S: Storage<M::Command>, M: StateMachine> Leader<S, M> {
	pub fn new(term: Term, shared: &Shared<S, M>) -> Self {
		let heartbeat_interval = shared.config().intervals().heartbeat_interval;
		let heartbeat_timeout = Box::pin(sleep(heartbeat_interval));

		// Notify the group that we are the new leader. This will cause followers to
		// update their leader information and start following us.
		shared
			.group()
			.when
			.update_leader(Some(shared.group().local_id()));

		Self {
			term,
			heartbeat_timeout,
			heartbeat_interval,
			wakers: Vec::new(),
			pending_commands: Vec::new(),
			_phantom: PhantomData,
		}
	}

	/// Returns the current term.
	pub const fn term(&self) -> Term {
		self.term
	}
}

impl<S: Storage<M::Command>, M: StateMachine> Leader<S, M> {
	/// As a leader, we send `AppendEntries` with new log entries or as heartbeats
	/// to all followers. We also handle client requests for log mutations.
	///
	/// If we receive an `AppendEntries` from another leader with a higher term,
	/// we step down to follower state and follow that leader.
	///
	/// Returns `Poll::Ready(ControlFlow::Break(new_role))` if the role should
	/// transition to a new state (e.g., follower) or `Poll::Pending` if it
	/// should continue waiting in the leader state.
	pub fn poll_next_tick(
		&mut self,
		cx: &mut Context,
		shared: &Shared<S, M>,
	) -> Poll<ControlFlow<Role<S, M>>> {
		if self.heartbeat_timeout.as_mut().poll(cx).is_ready() {
			let (prev_term, prev_index) = shared.log().last();
			let heartbeat = AppendEntries::<M::Command> {
				term: self.term,
				leader: shared.group().local_id(),
				prev_log_index: prev_index,
				prev_log_term: prev_term,
				entries: Vec::new(),
				leader_commit: shared.log().committed().1,
			};

			shared
				.group()
				.bonds
				.broadcast_raft_message::<M>(Message::AppendEntries(heartbeat));

			self.reset_heartbeat_timeout();
			return Poll::Ready(ControlFlow::Continue(()));
		}

		// store a waker to this task so we can wait it up when new client commands
		// are added before the next heartbeat timeout elapses.
		self.wakers.push(cx.waker().clone());

		Poll::Pending
	}

	pub fn receive(
		&self,
		message: Message<M::Command>,
		sender: PeerId,
		shared: &Shared<S, M>,
	) {
		let Message::AppendEntriesResponse(_response) = message else {
			tracing::warn!(
				local_term = self.term(),
				message_term = message.term(),
				group = %Short(shared.group().group_id()),
				network = %Short(shared.group().network_id()),
				sender = %Short(sender),
				"unexpected message type received by leader",
			);
			return;
		};
	}
}

impl<S: Storage<M::Command>, M: StateMachine> Leader<S, M> {
	fn reset_heartbeat_timeout(&mut self) {
		let next_heartbeat = Instant::now() + self.heartbeat_interval;
		self.heartbeat_timeout.as_mut().reset(next_heartbeat.into());

		for waker in self.wakers.drain(..) {
			waker.wake();
		}
	}
}
