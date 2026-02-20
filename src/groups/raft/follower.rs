use {
	crate::{
		PeerId,
		groups::{
			CommandError,
			CommittedQueryResult,
			Index,
			IndexRange,
			QueryError,
			StateMachine,
			StateSync,
			StateSyncSession,
			Storage,
			Term,
			raft::{
				Message,
				candidate::Candidate,
				protocol::{AppendEntries, AppendEntriesResponse, Forward, Vote},
				role::{Role, RoleHandlerError},
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
	pending_commands: HashMap<u64, oneshot::Sender<IndexRange>>,

	/// Tracks the pending forwarded queries from this follower to the leader.
	pending_queries: HashMap<u64, oneshot::Sender<CommittedQueryResult<M>>>,

	/// Channel for tracking forwarded commands that have expired without
	/// receiving a response from the leader within the forward timeout duration.
	expired_commands: UnboundedChannel<u64>,

	/// Channel for tracking forwarded queries that have expired without
	/// receiving a response from the leader within the forward timeout duration.
	expired_queries: UnboundedChannel<u64>,

	/// Optional state for the catch-up process when a follower is lagging behind
	/// the leader and needs to fetch missing log entries from peers before it
	/// can be in sync with the leader and transition to online status. This is
	/// set when we receive an `AppendEntries` message from the leader that is
	/// inconsistent with our local log state.
	catchup: Option<<M::StateSync as StateSync>::Session>,

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
		let mut election_timeout = shared.config().consensus().election_timeout();

		if term.is_zero() {
			// for the initial term, we introduce an additional bootstrap delay to
			// give all nodes enough time to start up and discover each other before
			// triggering the first election.
			election_timeout += shared.config().consensus().bootstrap_delay;
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
			pending_commands: HashMap::default(),
			pending_queries: HashMap::default(),
			expired_commands: UnboundedChannel::default(),
			expired_queries: UnboundedChannel::default(),
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
				Poll::Ready(cursor) => {
					assert_eq!(cursor, shared.storage.last());

					// we're caught up and in sync with the leader, turn off catch-up
					// mode and start watching the election timeout again for any
					// potential leader failures.
					self.catchup = None;

					tracing::info!(
						log_at = %cursor,
						term = %self.term(),
						group = %Short(shared.group_id()),
						network = %Short(shared.network_id()),
						"state sync complete, back online"
					);

					// signal updated log position to public api observers.
					shared.update_log_pos(cursor);
				}
				// we're still catching up
				Poll::Pending => return Poll::Pending,
			}
		}

		if self.election_timeout.as_mut().poll(cx).is_ready() {
			// election timeout elapsed without hearing from a leader, so we start
			// a new election by transitioning to candidate state
			return Poll::Ready(ControlFlow::Break(
				Candidate::new(self.term.next(), shared).into(),
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
	/// - `Sync::AvailabilityResponse` when in catch-up mode.
	/// - `Sync::FetchEntriesResponse` when in catch-up mode.
	///
	/// All other messages (e.g. `RequestVote` from candidates) are handled in the
	/// shared logic that is common to all roles and will be returned as an error
	/// if they are unexpected for this role.
	pub fn receive_protocol_message<S: Storage<M::Command>>(
		&mut self,
		message: Message<M>,
		sender: PeerId,
		shared: &mut Shared<S, M>,
	) -> Result<(), RoleHandlerError<M>> {
		match message {
			Message::AppendEntries(request) => {
				self.on_append_entries(request, sender, shared);
			}
			Message::Forward(Forward::CommandAck {
				request_id,
				assigned: assigned_indices,
			}) => {
				self.on_forward_command_response(request_id, assigned_indices);
			}
			Message::Forward(Forward::QueryResponse {
				request_id,
				result,
				position,
			}) => {
				self.on_forward_query_response(request_id, result, position);
			}

			// state-sync related messages
			Message::StateSync(message) => {
				if let Some(catchup) = self.catchup.as_mut() {
					catchup.receive(message, sender, shared);
				}
			}

			message => {
				return Err(RoleHandlerError::<M>::Unexpected(message));
			}
		}

		Ok(())
	}

	/// Resets the election timeout.
	///
	/// Raft paper 5.2: If election timeout elapses without receiving
	/// `AppendEntries` RPC from current leader or granting vote to candidate:
	/// convert to candidate
	///
	/// Voting happens at the role level, so we need to expose this method to
	/// allow `Role` to reset our timeout as a follower in `maybe_cast_vote`.
	pub fn reset_election_timeout<S: Storage<M::Command>>(
		&mut self,
		shared: &Shared<S, M>,
	) {
		let next_election_timeout =
			Instant::now() + shared.consensus().election_timeout();

		self
			.election_timeout
			.as_mut()
			.reset(next_election_timeout.into());
	}

	/// Forwards the command to the current leader and returns a future that
	/// resolves when the leader acknowledges the command with the assigned log
	/// index in a `Forward::ExecuteAck`.
	pub fn forward_commands<S: Storage<M::Command>>(
		&mut self,
		commands: Vec<M::Command>,
		shared: &Shared<S, M>,
	) -> BoxPinFut<Result<IndexRange, CommandError<M>>> {
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
			if self.pending_commands.contains_key(&id) {
				continue;
			}
			break id;
		};

		// send the command to the current leader
		let message = Message::Forward(Forward::Command {
			commands: commands.clone(), // todo: avoid cloning!
			request_id: Some(request_id),
		});

		let (forward_ack_tx, forward_ack_rx) = oneshot::channel();
		self.pending_commands.insert(request_id, forward_ack_tx);
		shared.bonds().send_raft_to::<M>(&message, leader);

		let expired_sender = self.expired_commands.sender().clone();
		let forward_timeout = shared.consensus().forward_timeout;

		async move {
			if let Ok(Ok(assigned)) = timeout(forward_timeout, forward_ack_rx).await {
				Ok(assigned)
			} else {
				expired_sender.send(request_id).ok();
				Err(CommandError::Offline(commands))
			}
		}
		.pin()
	}

	/// Forwards a query with strong consistency requirement to the current leader
	/// and returns the query result from leader's state machine and the
	/// committed log position at which the query was executed.
	pub fn forward_query<S: Storage<M::Command>>(
		&mut self,
		query: M::Query,
		shared: &Shared<S, M>,
	) -> BoxPinFut<Result<CommittedQueryResult<M>, QueryError<M>>> {
		let Some(leader) = self.leader() else {
			// if the follower does not know who's the current leader, return an error
			// with the query.
			return ready(Err(QueryError::Offline(query))).pin();
		};

		// generate a random request id
		let request_id: u64 = loop {
			let id = rand::random();
			if self.pending_queries.contains_key(&id) {
				continue;
			}
			break id;
		};

		// send the query to the current leader
		let message = Message::Forward(Forward::Query {
			query: query.clone(), // todo: avoid cloning!
			request_id,
		});

		let (response_tx, response_rx) = oneshot::channel();
		self.pending_queries.insert(request_id, response_tx);
		shared.bonds().send_raft_to::<M>(&message, leader);

		let expired_sender = self.expired_queries.sender().clone();
		let query_timeout = shared.consensus().query_timeout;

		async move {
			if let Ok(Ok(response)) = timeout(query_timeout, response_rx).await {
				Ok(response)
			} else {
				expired_sender.send(request_id).ok();
				Err(QueryError::Offline(query))
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
		self.reset_election_timeout(shared);

		// check the consistency of the incoming `AppendEntries` message with our
		// local log state and if we need to catch up before we can confirm this
		// message and update our log to match the leader's log.
		let consistent =
			match shared.storage.term_at(request.prev_log_position.index()) {
				// The entry at `prev_log_index` matches the leader's term.
				// If we have entries beyond this point, check whether the
				// first new entry conflicts with what we already have.
				// The leader's log always wins — truncate from the conflict.
				Some(local_term) if local_term == request.prev_log_position.term() => {
					if let Some(first) = request.entries.first() {
						let next_index = request.prev_log_position.index().next();
						if let Some(existing_term) = shared.storage.term_at(next_index)
							&& existing_term != first.term
						{
							shared.storage.truncate(next_index);
						}
					}
					true
				}

				// Term conflict at `prev_log_index` - this entry came from a deposed
				// leader. Truncate it and everything after it. This creates a gap, so
				// we will need to catch up before we can append and confirm.
				Some(_) => {
					shared.storage.truncate(request.prev_log_position.index());
					false
				}

				// No entry at `prev_log_index` means our log is behind the leader's
				// log. Need to catch up before we can append and confirm.
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
			let entries = request
				.entries
				.into_iter()
				.map(|entry| (entry.command, entry.term))
				.collect();
			if let Some(catchup) = self.catchup.as_mut() {
				catchup.buffer(request.prev_log_position, entries, shared);
			} else {
				tracing::info!(
					leader_pos = %request.prev_log_position,
					local_pos = %shared.storage.last(),
					term = %self.term(),
					group = %Short(shared.group_id()),
					network = %Short(shared.network_id()),
					"starting state sync"
				);

				self.catchup = Some(shared.create_sync_session(
					request.prev_log_position, //
					request.leader_commit,
					entries,
				));
			}
		}
	}

	/// Handles an incoming `Forward::ExecuteAck` message from the leader in
	/// response to a command that this follower forwarded to the leader. Signals
	/// the assignment of a log index to the forwarded command.
	fn on_forward_command_response(
		&mut self,
		request_id: u64,
		assigned: IndexRange,
	) {
		if let Some(ack) = self.pending_commands.remove(&request_id) {
			let _ = ack.send(assigned);
		}
	}

	/// Handles an incoming `Forward::QueryResponse` message from the leader in
	/// response to a query with strong consistency requirement that this follower
	/// sent to the leader.
	fn on_forward_query_response(
		&mut self,
		request_id: u64,
		result: M::QueryResult,
		at_position: Index,
	) {
		if let Some(response) = self.pending_queries.remove(&request_id) {
			let _ = response.send(CommittedQueryResult {
				result,
				at_position,
			});
		}
	}

	/// Periodically cleans up any pending forwarded commands and queries that
	/// have expired without receiving a response from the leader within the
	/// forward timeout.
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
					self.pending_commands.remove(&id);
				}
			}
		}

		if !self.expired_queries.is_empty() {
			let count = self.expired_queries.len();
			let mut ids = Vec::with_capacity(count);

			if self
				.expired_queries
				.poll_recv_many(cx, &mut ids, count)
				.is_ready()
			{
				for id in ids {
					self.pending_queries.remove(&id);
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
			let start_index = request.prev_log_position.index().next();
			for (i, entry) in request.entries.into_iter().enumerate() {
				let index = start_index + i;
				if shared.storage.term_at(index) == Some(entry.term) {
					// already have this entry (and we verified no conflicts above), so
					// skip it.
					continue;
				}

				shared.storage.append(entry.command, entry.term);
				appended_count += 1;
			}

			// confirm to the leader that we have appended the entries and report the
			// index of our last log entry
			let local_position = shared.storage.last();
			shared.bonds().send_raft_to::<M>(
				&Message::AppendEntriesResponse(AppendEntriesResponse {
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
		let local_log_pos = shared.storage.last();
		let prev_committed = shared.committed();
		let leader_committed = request.leader_commit.min(local_log_pos.index());
		let mut new_committed = prev_committed;

		if prev_committed < leader_committed {
			new_committed = shared.commit_up_to(leader_committed);
			if prev_committed < new_committed {
				// Signal to public api observers that the committed index has advanced
				shared.update_committed(new_committed);
				shared.prune_safe_prefix();
			}
		}

		if prev_committed != new_committed || appended_count > 0 {
			tracing::trace!(
				committed_ix = %new_committed,
				new_entries = appended_count,
				local_log = %shared.storage.last(),
				term = %self.term(),
				group = %Short(shared.group_id()),
				network = %Short(shared.network_id()),
			);
		}
	}
}
