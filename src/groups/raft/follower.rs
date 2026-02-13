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
				catchup::Catchup,
				protocol::{AppendEntries, AppendEntriesResponse, Forward, Sync, Vote},
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

	/// Optional state for the catch-up process when a follower is lagging behind
	/// the leader and needs to fetch missing log entries from peers before it
	/// can be in sync with the leader and transition to online status. This is
	/// set when we receive an `AppendEntries` message from the leader that is
	/// inconsistent with our local log state.
	catchup: Option<Catchup<M>>,

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
			catchup: None,
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
	///
	/// When the follower is in catch-up mode, it suppresses the election timeout
	/// and focuses on catching up with the leader's log. During this time, it
	/// will not trigger elections even if it does not receive messages from the
	/// leader, allowing other in-sync nodes to trigger elections if the leader
	/// is unavailable.
	pub fn poll_next_tick<S: Storage<M::Command>>(
		&mut self,
		cx: &mut Context<'_>,
		shared: &mut Shared<S, M>,
	) -> Poll<ControlFlow<Role<M>>> {
		self.clean_expired_forwarded_commands(cx);

		if let Some(catchup) = self.catchup.as_mut() {
			// if we're in catch-up mode, suppress the election timeouts because we
			// can't win elections anyway with a stale log. Focus on catching up and
			// let other in-sync nodes trigger elections if the leader is
			// unavailable.
			match catchup.poll_next_tick(cx, shared) {
				Poll::Ready(ControlFlow::Break(())) => {
					// we're caught up and in sync with the leader, turn off catch-up
					// mode and start watching the election timeout again for any
					// potential leader failures.
					self.catchup = None;
				}
				// we're still catching up
				Poll::Ready(ControlFlow::Continue(())) => {
					return Poll::Ready(ControlFlow::Continue(()));
				}
				// we're still catching up
				Poll::Pending => return Poll::Pending,
			}
		}

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
	/// role-specific logic are `AppendEntries` and `Forward::ExecuteAck` from
	/// the leader.
	///
	/// In follower mode we care about the following message:
	/// - `AppendEntries` from the leader
	/// - `Forward::ExecuteAck` from the leader.
	/// - `Sync::DiscoveryResponse` when in catch-up mode.
	/// - `Sync::FetchEntriesResponse` when in catch-up mode.
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
			Message::Forward(Forward::ExecuteAck {
				request_id,
				log_index,
			}) => {
				self.on_forward_command_response(request_id, log_index);
			}

			// Peers response to our `DiscoveryRequest` messages during the catch-up.
			Message::Sync(Sync::DiscoveryResponse { available }) => {
				if let Some(catchup) = self.catchup.as_mut() {
					catchup.record_availability(available, sender);
				}
			}

			// Peers response to our `FetchEntriesRequest` messages during catch-up
			Message::Sync(Sync::FetchEntriesResponse { range, entries }) => {
				if let Some(catchup) = self.catchup.as_mut() {
					catchup.receive_entries(sender, range, entries);
				}
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
	/// index in a `Forward::ExecuteAck`.
	pub fn forward_commands<S: Storage<M::Command>>(
		&mut self,
		commands: Vec<M::Command>,
		shared: &Shared<S, M>,
	) -> BoxPinFut<Result<Index, CommandError<M>>> {
		let Some(leader) = self.leader() else {
			// if the follower does not know who's the current leader (e.g. because
			// elections are in progress or because it is still in the initial offline
			// state before hearing from a leader), it cannot accept the command, so
			// we return a future that resolves with with Offline error.
			return ready(Err(CommandError::Offline(commands))).pin();
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
		let message = Message::Forward(Forward::Execute {
			commands: commands.clone(),
			request_id: Some(request_id),
		});

		let (forward_ack_tx, forward_ack_rx) = oneshot::channel();
		self.forwarded_commands.insert(request_id, forward_ack_tx);
		shared.bonds().send_raft_to::<M>(message, leader);

		let expired_sender = self.expired_commands.sender().clone();
		let forward_timeout = shared.intervals().forward_timeout;

		async move {
			if let Ok(Ok(index)) = timeout(forward_timeout, forward_ack_rx).await {
				Ok(index)
			} else {
				expired_sender.send(request_id).ok();
				Err(CommandError::Offline(commands))
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
		let consistent = match shared.log.term_at(request.prev_log_position.index())
		{
			// The entry at `prev_log_index` matches the leader's term.
			// If we have entries beyond this point, check whether the
			// first new entry conflicts with what we already have.
			// The leader's log always wins — truncate from the conflict.
			Some(local_term) if local_term == request.prev_log_position.term() => {
				if let Some(first) = request.entries.first() {
					let next_index = request.prev_log_position.index() + 1;
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
				shared.log.truncate(request.prev_log_position.index());
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
			// log is inconsistent wit the leader, go into offline mode until we're
			// caught up.
			shared.set_offline();

			// buffer the leader's inflight entries while we're catching up on the
			// log.
			self
				.catchup
				.get_or_insert_with(|| {
					Catchup::<M>::new(request.prev_log_position, shared)
				})
				.buffer_current_entries(request.prev_log_position, request.entries);
		}
	}

	/// Handles an incoming `Forward::ExecuteAck` message from the leader in
	/// response to a command that this follower forwarded to the leader. Signals
	/// the assignment of a log index to the forwarded command.
	fn on_forward_command_response(&mut self, request_id: u64, log_index: Index) {
		if let Some(ack) = self.forwarded_commands.remove(&request_id) {
			let _ = ack.send(log_index);
		}
	}

	/// Periodically cleans up any pending forwarded commands that have expired
	/// without receiving a response from the leader within the forward timeout.
	fn clean_expired_forwarded_commands(&mut self, cx: &mut Context<'_>) {
		if !self.expired_commands.is_empty() {
			let count = self.expired_commands.len();
			let mut ids = Vec::with_capacity(count);

			if self
				.expired_commands
				.poll_recv_many(cx, &mut ids, count)
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
		let mut appended_count = 0;
		if !request.entries.is_empty() {
			// the log is consistent up to `prev_log_index`. Append the leader's new
			// entries, skipping any we already have, which handles idempotent
			// redelivery).
			let start_index = request.prev_log_position.index() + 1;
			for (i, entry) in request.entries.into_iter().enumerate() {
				let index = start_index + i as u64;
				if shared.log.term_at(index) == Some(entry.term) {
					// already have this entry (and we verified no conflicts above), so
					// skip it.
					continue;
				}

				shared.log.append(entry.command, entry.term);
				appended_count += 1;
			}

			// confirm to the leader that we have appended the entries and report the
			// index of our last log entry
			let local_position = shared.log.last();
			shared.bonds().send_raft_to::<M>(
				Message::AppendEntriesResponse(AppendEntriesResponse {
					term: self.term(),
					vote: Vote::Granted,
					last_log_index: local_position.index(),
				}),
				sender,
			);
		}

		// advance local commit index to the minimum of the leader's commit index
		// and the index of our last log entry, which ensures we only commit
		// entries that are both replicated to a majority
		let local_log_pos = shared.log.last();
		let prev_committed = shared.log.committed();
		let leader_committed = request.leader_commit.min(local_log_pos.index());
		let mut new_committed = prev_committed;

		if prev_committed < leader_committed {
			new_committed = shared.log.commit_up_to(leader_committed);
			if prev_committed < new_committed {
				// Signal to public api observers that the committed index has advanced
				shared.when().update_committed(new_committed);
			}
		}

		if prev_committed != new_committed || appended_count > 0 {
			tracing::trace!(
				committed_ix = new_committed,
				new_entries = appended_count,
				local_log = %shared.log.last(),
				term = self.term(),
				group = %Short(shared.group_id()),
				network = %Short(shared.network_id()),
			);
		}
	}
}
