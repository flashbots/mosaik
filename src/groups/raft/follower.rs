use {
	crate::{
		PeerId,
		groups::{
			CommandError,
			Index,
			log::{StateMachine, Storage, Term},
			raft::{
				Message,
				candidate::Candidate,
				protocol::{
					AppendEntries,
					AppendEntriesResponse,
					ForwardCommand,
					ForwardCommandResponse,
					Vote,
				},
				role::Role,
				shared::Shared,
			},
		},
		primitives::{BoxPinFut, InternalFutureExt, Short, UnboundedChannel},
	},
	core::{
		future::ready,
		marker::PhantomData,
		ops::ControlFlow,
		pin::Pin,
		task::{Context, Poll},
	},
	std::{collections::HashMap, time::Instant},
	tokio::{
		sync::oneshot,
		time::{Sleep, sleep, timeout},
	},
};

/// In the follower role, the node is passive and responds to messages from
/// candidates and leaders. If the election timeout elapses without receiving
/// `AppendEntries` from the current leader, the follower transitions to
/// leader candidate state and starts a new election.
#[derive(Debug)]
pub struct Follower<M: StateMachine> {
	/// The current term for this node.
	term: Term,

	/// The current leader that this follower is following (if any). This is
	/// updated whenever we receive a valid `AppendEntries` message from a
	/// leader.
	leader: Option<PeerId>,

	/// The election timeout for this follower. If we do not receive any
	/// messages from a valid leader within this timeout, we will transition to
	/// candidate state and start a new election.
	election_timeout: Pin<Box<Sleep>>,

	/// Tracks the pending forwarded commands from this follower to the leader
	/// that are awaiting acknowledgment with the assigned log index from the
	/// leader's `ForwardCommandResponse`.
	forwarded_commands: HashMap<u64, oneshot::Sender<Index>>,

	/// Channel for tracking forwarded commands that have expired without
	/// receiving a response from the leader within the forward timeout duration.
	expired_commands: UnboundedChannel<u64>,

	#[doc(hidden)]
	_marker: PhantomData<M>,
}

impl<M: StateMachine> Follower<M> {
	/// Creates a new follower role for the specified term
	pub fn new<S: Storage<M::Command>>(
		term: Term,
		leader: Option<PeerId>,
		shared: &Shared<S, M>,
	) -> Self {
		let mut election_timeout = shared.config().intervals().election_timeout();

		if term == 0 {
			// for the initial term, we introduce an additional bootstrap delay to
			// give all nodes enough time to start up and discover each other before
			// triggering the first election.
			election_timeout += shared.config().intervals().bootstrap_delay;
		}

		if let Some(leader) = leader {
			// If we are transitioning to a follower role with a known leader,
			shared.update_leader(Some(leader));
		}

		// after transitioning to follower role, we are initially offline until we
		// receive a valid `AppendEntries` message from a leader that confirms
		// that we are in sync with the leader's log and can be considered online.
		shared.set_offline();

		Self {
			term,
			leader,
			forwarded_commands: HashMap::default(),
			expired_commands: UnboundedChannel::default(),
			election_timeout: Box::pin(sleep(election_timeout)),
			_marker: PhantomData,
		}
	}

	/// Returns the current term of this follower.
	pub const fn term(&self) -> Term {
		self.term
	}

	/// Returns the current leader that this follower is following (if known).
	pub const fn leader(&self) -> Option<PeerId> {
		self.leader
	}
}

impl<M: StateMachine> Follower<M> {
	/// As a follower, we wait for messages from leaders or candidates.
	/// If no messages are received within the election timeout, we transition to
	/// a candidate state and start a new election.
	///
	/// Returns `Poll::Ready(ControlFlow::Break(new_role))` if the role should
	/// transition to a new state (e.g., candidate) or `Poll::Pending` if it
	/// should continue waiting in the follower state.
	pub fn poll_next_tick<S: Storage<M::Command>>(
		&mut self,
		cx: &mut Context<'_>,
		shared: &mut Shared<S, M>,
	) -> Poll<ControlFlow<Role<M>>> {
		self.clean_expired_forwarded_commands(cx);

		if self.election_timeout.as_mut().poll(cx).is_ready() {
			// election timeout elapsed without hearing from a leader, so we start
			// a new election by transitioning to candidate state
			return Poll::Ready(ControlFlow::Break(
				Candidate::new(self.term + 1, shared).into(),
			));
		}

		Poll::Pending
	}

	/// In follower mode the only two messages that are handled in the
	/// role-specific logic are `AppendEntries` and `ForwardCommandResponse` from
	/// the leader.
	///
	/// All other messages (e.g. `RequestVote` from candidates) are handled in the
	/// shared logic that is common to all roles.
	pub fn receive_protocol_message<S: Storage<M::Command>>(
		&mut self,
		message: Message<M::Command>,
		sender: PeerId,
		shared: &mut Shared<S, M>,
	) {
		match message {
			Message::AppendEntries(request) => {
				self.on_append_entries(request, sender, shared);
			}
			Message::ForwardCommandResponse(response) => {
				self.on_forward_command_response(response);
			}
			_ => {
				tracing::warn!(
					term = self.term(),
					sender = %Short(sender),
					group = %Short(shared.group_id()),
					network = %Short(shared.network_id()),
					"unexpected message in follower role: {message:?}",
				);
			}
		}
	}

	/// Forwards the command to the current leader and returns a future that
	/// resolves when the leader acknowledges the command with the assigned log
	/// index in a `ForwardCommandResponse`.
	pub fn forward_command<S: Storage<M::Command>>(
		&mut self,
		command: M::Command,
		shared: &Shared<S, M>,
	) -> BoxPinFut<Result<Index, CommandError<M>>> {
		let Some(leader) = self.leader() else {
			// if the follower does not know who's the current leader (e.g. because
			// elections are in progress or because it is still in the initial offline
			// state before hearing from a leader), it cannot accept the command, so
			// we return a future that resolves with with Offline error.
			return ready(Err(CommandError::Offline(command))).pin();
		};

		// generate a random request id
		let request_id: u64 = loop {
			let id = rand::random();
			if self.forwarded_commands.contains_key(&id) {
				continue;
			}
			break id;
		};

		// send the command to the current leader
		let message = Message::ForwardCommand(ForwardCommand {
			command: command.clone(),
			request_id: Some(request_id),
		});

		let (forward_ack_tx, forward_ack_rx) = oneshot::channel();
		self.forwarded_commands.insert(request_id, forward_ack_tx);
		shared.bonds().send_raft_message_to::<M>(message, leader);

		let expired_sender = self.expired_commands.sender().clone();
		let forward_timeout = shared.intervals().forward_timeout;

		async move {
			if let Ok(Ok(index)) = timeout(forward_timeout, forward_ack_rx).await {
				Ok(index)
			} else {
				expired_sender.send(request_id).ok();
				Err(CommandError::Offline(command))
			}
		}
		.pin()
	}

	/// Handles an incoming `AppendEntries` message from a leader.
	fn on_append_entries<S: Storage<M::Command>>(
		&mut self,
		request: AppendEntries<M::Command>,
		sender: PeerId,
		shared: &mut Shared<S, M>,
	) {
		// signal to public api status listeners a potential update
		// to the current leader for this follower.
		self.leader = Some(request.leader);
		shared.update_leader(Some(request.leader));

		// each valid `AppendEntries` message from a leader resets the election
		// timeout and updates the current leader for this follower
		let next_election_timeout =
			Instant::now() + shared.intervals().election_timeout();

		self
			.election_timeout
			.as_mut()
			.reset(next_election_timeout.into());

		// check the consistency of the incoming `AppendEntries` message with our
		// local log state and if we need to catch up before we can confirm this
		// message and update our log to match the leader's log.
		let consistent = match shared.log.term_at(request.prev_log_index) {
			// The entry at `prev_log_index` matches the leader's term.
			// If we have entries beyond this point, check whether the
			// first new entry conflicts with what we already have.
			// The leader's log always wins — truncate from the conflict.
			Some(local_term) if local_term == request.prev_log_term => {
				if let Some(first) = request.entries.first() {
					let next_index = request.prev_log_index + 1;
					if let Some(existing_term) = shared.log.term_at(next_index) {
						if existing_term != first.term {
							shared.log.truncate(next_index);
						}
					}
				}
				true
			}

			// Term conflict at `prev_log_index` - this entry came from a deposed
			// leader. Truncate it and everything after it. This creates a gap, so we
			// will need to catch up before we can append and confirm.
			Some(_) => {
				shared.log.truncate(request.prev_log_index);
				false
			}

			// No entry at `prev_log_index` means our log is behind the leader's log.
			// Need to catch up before we can append and confirm.
			None => false,
		};

		if consistent {
			// we're in sync, append to local store, commit up to the leader's commit
			// index, and respond with success to the leader.
			shared.set_online();
			self.accept_in_sync_entries(request, sender, shared);
		} else {
			// Log is inconsistent - we are missing entries before `prev_log_index`.
			// Buffer the incoming entries and enter catch-up mode to fetch the gap
			// from peers.
			shared.set_offline();
			todo!("implement follower catch-up")
		}
	}

	/// Handles an incoming `ForwardCommandResponse` message from the leader in
	/// response to a command that this follower forwarded to the leader. Signals
	/// the assignment of a log index to the forwarded command.
	fn on_forward_command_response(&mut self, response: ForwardCommandResponse) {
		if let Some(ack) = self.forwarded_commands.remove(&response.request_id) {
			let _ = ack.send(response.log_index);
		}
	}

	/// Periodically cleans up any pending forwarded commands that have expired
	/// without receiving a response from the leader within the forward timeout.
	fn clean_expired_forwarded_commands(&mut self, cx: &mut Context<'_>) {
		if !self.expired_commands.is_empty() {
			let mut ids = Vec::with_capacity(self.expired_commands.len());
			if self
				.expired_commands
				.poll_recv_many(cx, &mut ids, 10)
				.is_ready()
			{
				for id in ids {
					self.forwarded_commands.remove(&id);
				}
			}
		}
	}

	/// Called when this follower's log state is in sync with the leader's log and
	/// we want to append and commit the entries from the leader's `AppendEntries`
	/// message and update our log state to match the leader's log.
	fn accept_in_sync_entries<S: Storage<M::Command>>(
		&self,
		request: AppendEntries<M::Command>,
		sender: PeerId,
		shared: &mut Shared<S, M>,
	) {
		if !request.entries.is_empty() {
			// the log is consistent up to `prev_log_index`. Append the leader's new
			// entries, skipping any we already have, which handles idempotent
			// redelivery).
			let start_index = request.prev_log_index + 1;
			for (i, entry) in request.entries.into_iter().enumerate() {
				let index = start_index + i as u64;
				if shared.log.term_at(index) == Some(entry.term) {
					// already have this entry (and we verified no conflicts above), so
					// skip it.
					continue;
				}

				shared.log.append(entry.command, entry.term);
			}

			// confirm to the leader that we have appended the entries and report the
			// index of our last log entry
			let (_, last_log_index) = shared.log.last();
			shared.bonds().send_raft_message_to::<M>(
				Message::AppendEntriesResponse(AppendEntriesResponse {
					term: self.term(),
					vote: Vote::Granted,
					last_log_index,
				}),
				sender,
			);
		}

		// advance local commit index to the minimum of the leader's commit index
		// and the index of our last log entry, which ensures we only commit
		// entries that are both replicated to a majority
		let (_, last_log_index) = shared.log.last();
		let prev_committed = shared.log.committed();
		let leader_committed = request.leader_commit.min(last_log_index);
		let mut new_committed = prev_committed;

		if prev_committed < leader_committed {
			new_committed = shared.log.commit_up_to(leader_committed);
			if prev_committed < new_committed {
				// Signal to public api observers that the committed index has advanced
				shared.when().update_committed(new_committed);
			}
		}

		tracing::trace!(
			committed_ix = new_committed,
			log_len = last_log_index,
			term = self.term(),
			group = %Short(shared.group_id()),
			network = %Short(shared.network_id()),
		);
	}
}
