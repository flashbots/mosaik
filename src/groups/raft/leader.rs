use {
	crate::{
		PeerId,
		groups::{
			Index,
			log::{StateMachine, Storage, Term},
			raft::{
				Message,
				protocol::{AppendEntries, Forward, LogEntry, Vote},
				role::{Role, RoleHandlerError},
				shared::Shared,
			},
		},
		primitives::{FmtIter, Short, UnboundedChannel},
	},
	core::{
		cmp::Reverse,
		iter::once,
		ops::ControlFlow,
		pin::Pin,
		task::{Context, Poll},
		time::Duration,
	},
	std::{
		collections::{HashMap, HashSet},
		time::Instant,
	},
	tokio::time::{Sleep, sleep},
};

/// In the leader role, the node is active and handles log-mutating requests
/// from clients, replicates log entries, and sends periodic heartbeats to
/// followers if no log entries are being replicated within the configured
/// heartbeat interval. If the leader receives an `AppendEntries` message from
/// another leader with a higher term, it steps down to follower state and
/// follows that leader.
#[derive(Debug)]
pub struct Leader<M: StateMachine> {
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
	client_commands: UnboundedChannel<M::Command>,

	/// The current voting committee that is used to determine the quorum for
	/// elections and log replication.
	committee: Committee,

	/// Wakers for tasks that are waiting for the leader to either send
	/// heartbeats or replicate log entries.
	wakers: Vec<std::task::Waker>,
}

impl<M: StateMachine> Leader<M> {
	/// Transitions into a new leader role for a new term.
	///
	/// This will also inform the new leader about the peers that have granted
	/// their vote in the election, which optimizes the formation of the initial
	/// quorum.
	pub fn new(
		term: Term,
		voted_by: HashSet<PeerId>,
		shared: &Shared<impl Storage<M::Command>, M>,
	) -> Self {
		// initialize the voting committee with the peers that granted their vote to
		// us in the election.
		let committee = Committee::new(voted_by, shared);

		let heartbeat_interval = shared.config().intervals().heartbeat_interval;
		let heartbeat_timeout = Box::pin(sleep(heartbeat_interval));

		// Notify the group that we are the new leader. This will cause followers to
		// update their leader information and start following us.
		shared.update_leader(Some(shared.local_id()));
		shared.set_online();

		Self {
			term,
			heartbeat_timeout,
			heartbeat_interval,
			committee,
			wakers: Vec::new(),
			client_commands: UnboundedChannel::default(),
		}
	}

	/// Returns the current term.
	pub const fn term(&self) -> Term {
		self.term
	}
}

impl<M: StateMachine> Leader<M> {
	/// As a leader, we send `AppendEntries` with new log entries or as heartbeats
	/// to all followers. We also handle client requests for log mutations.
	///
	/// If we receive an `AppendEntries` from another leader with a higher term,
	/// we step down to follower state and follow that leader.
	///
	/// Returns `Poll::Ready(ControlFlow::Break(new_role))` if the role should
	/// transition to a new state (e.g., follower) or `Poll::Pending` if it
	/// should continue waiting in the leader state.
	pub fn poll_next_tick<S: Storage<M::Command>>(
		&mut self,
		cx: &mut Context,
		shared: &mut Shared<S, M>,
	) -> Poll<ControlFlow<Role<M>>> {
		if self.poll_pending_entries(cx, shared).is_ready() {
			// this tick was spent publishing pending client commands to followers
			return Poll::Ready(ControlFlow::Continue(()));
		}

		if self.poll_next_heartbeat(cx, shared).is_ready() {
			// this tick was spent sending a heartbeat to followers
			// since no commands were published, and the heartbeat timeout elapsed.
			return Poll::Ready(ControlFlow::Continue(()));
		}

		// store a waker to this task so we can wait it up when new client commands
		// are added or when the heartbeat timeout elapses to trigger the next tick.
		self.wakers.push(cx.waker().clone());

		Poll::Pending
	}

	/// As a leader we are only interested in receiving `AppendEntriesResponse`
	/// messages from followers to track their replication progress and update our
	/// commit index.
	///
	/// Returns Ok(()) if the message was handled by the leader, or Err(message)
	/// if the message was unexpected and was not handled by this role.
	pub fn receive_protocol_message<S: Storage<M::Command>>(
		&mut self,
		message: Message<M::Command>,
		sender: PeerId,
		shared: &mut Shared<S, M>,
	) -> Result<(), RoleHandlerError<M>> {
		match message {
			// sent by followers in response to our `AppendEntries` messages.
			Message::AppendEntriesResponse(response) => {
				match response.vote {
					// The follower is up to date with our log and can be part of the
					// current voting committee. Record its vote and its log progress
					// for commit index tracking.
					Vote::Granted => {
						self.record_vote(sender, response.last_log_index, shared);
					}

					// The follower is not up to date with our log or has some other issue
					// that prevents it from being part of the current voting committee.
					// Remove it from the committee so it is not considered for quorum
					// calculations until it catches up with our log and can grant its
					// vote again.
					Vote::Abstained => self.record_no_vote(sender, true, shared),

					// the follower explicitly denied our `AppendEntries`
					Vote::Denied => self.record_no_vote(sender, false, shared),
				}
			}

			// if we're a leader and we're receiving a message with the same term from
			// another leader, this means that the group has two rival leaders, this
			// indicates a network partition. Trigger new elections with a higher
			// term.
			Message::AppendEntries(request) if request.term == self.term() => {
				return Err(RoleHandlerError::RivalLeader(request));
			}

			// sent by followers that are forwarding client commands to the leader.
			Message::Forward(Forward::Execute {
				commands,
				request_id,
			}) => {
				let log_index = self.enqueue_commands(commands, shared);

				if let Some(request_id) = request_id {
					// the follower is interested in knowing the log index assigned to
					// this command asap.
					shared.bonds().send_raft_to::<M>(
						Message::Forward(Forward::ExecuteAck {
							request_id,
							log_index,
						}),
						sender,
					);
				}
			}

			// all other message types are unexpected in the leader state. ignore.
			message => {
				return Err(RoleHandlerError::<M>::Unexpected(message));
			}
		}

		Ok(())
	}

	/// Adds a new client command to the list of pending commands that will be
	/// included in the next `AppendEntries` message sent to followers. This
	/// method is called when the leader receives a client request.
	///
	/// This will wake up any pending wakers that are waiting on the next leader
	/// tick.
	///
	/// Returns the index of the log entry that will be assigned to this command
	/// once it is appended to the log. This allows the caller to track the
	/// progress of their command and know when it has been committed and applied
	/// to the state machine.
	pub fn enqueue_commands<S: Storage<M::Command>>(
		&mut self,
		commands: Vec<M::Command>,
		shared: &Shared<S, M>,
	) -> Index {
		let last_index = shared.log.last().index();
		for command in commands {
			self.client_commands.send(command);
		}

		let expected_index = last_index + self.client_commands.len().into();

		for waker in self.wakers.drain(..) {
			waker.wake();
		}

		expected_index
	}

	/// Records a positive vote from a follower that has acknowledged our
	/// `AppendEntries` message and is up to date with our log.
	fn record_vote<S: Storage<M::Command>>(
		&mut self,
		follower: PeerId,
		log_index: Index,
		shared: &mut Shared<S, M>,
	) {
		// purge voters that are down and try to backfill voters from online
		// non-voters.
		self.committee.remove_dead_voters(shared);

		let prev_committed = shared.log.committed();
		let quorum_index = self.committee.record_vote(follower, log_index);

		if prev_committed != quorum_index {
			let new_committed = shared.log.commit_up_to(quorum_index);
			if new_committed != prev_committed {
				// advance the commit index up to the latest index that has reached a
				// quorum of voters in the committee.
				tracing::trace!(
					committed_ix = %new_committed,
					log_position = %shared.log.last(),
					group = %Short(shared.group_id()),
					network = %Short(shared.network_id()),
				);

				shared.update_committed(new_committed);
			}
		}
	}

	/// Records a negative vote from a follower that has not acknowledged our
	/// `AppendEntries` message or is not up to date with our log.
	fn record_no_vote<S: Storage<M::Command>>(
		&mut self,
		follower: PeerId,
		abstained: bool,
		shared: &mut Shared<S, M>,
	) {
		if abstained {
			// the follower didn't explicitly deny our `AppendEntries`, but it also
			// didn't grant its vote, which means it is not up to date with our log
			// and can't be part of the current voting committee until it catches up.
			// Remove it from the committee so it is not considered for quorum
			// calculations until it can grant its vote again.
			self.committee.remove(follower);
		}

		// purge voters that are down and try to backfill voters from online
		// non-voters.
		self.committee.remove_dead_voters(shared);

		// see if after purging dead voters and removing this non-voting follower,
		// we have reached a quorum with a smaller committee and can advance the
		// commit index.
		let prev_committed = shared.log.committed();
		let quorum_index = self.committee.highest_quorum_index();

		if prev_committed != quorum_index {
			let new_committed = shared.log.commit_up_to(quorum_index);
			if new_committed != prev_committed {
				// advance the commit index up to the latest index that has reached a
				// quorum of voters in the committee.
				tracing::trace!(
					committed = %new_committed,
					group = %Short(shared.group_id()),
					network = %Short(shared.network_id()),
				);

				shared.update_committed(new_committed);
			}
		}
	}
}

// internal impl
impl<M: StateMachine> Leader<M> {
	/// Checks if there are any pending client commands and publishes them to all
	/// followers as `AppendEntries` message.
	fn poll_pending_entries<S: Storage<M::Command>>(
		&mut self,
		cx: &mut Context,
		shared: &mut Shared<S, M>,
	) -> Poll<()> {
		if self.client_commands.is_empty() {
			// no pending client commands to publish, just wait for the next heartbeat
			// timeout or new client commands to arrive to publish `AppendEntries` to
			// followers.
			return Poll::Pending;
		}

		let count = self.client_commands.len();
		let mut entries = Vec::with_capacity(count);
		if self
			.client_commands
			.poll_recv_many(cx, &mut entries, count)
			.is_pending()
		{
			return Poll::Pending;
		}

		let prev_pos = shared.log.last();

		// append the new client commands to our log before broadcasting them to
		// followers but without committing them yet.
		for command in &entries {
			shared.log.append(command.clone(), self.term);
		}

		// signal log update to public api observers
		shared.update_log_pos(shared.log.last());

		// always vote for our own `AppendEntries` messages. If we are the
		// only voter in the committee, this will immediately commit the new log
		// entries.
		self.record_vote(
			shared.local_id(),
			prev_pos.index() + count.into(),
			shared,
		);

		let message = Message::AppendEntries(AppendEntries {
			term: self.term,
			leader_commit: shared.log.committed(),
			leader: shared.local_id(),
			prev_log_position: prev_pos,
			entries: entries
				.into_iter()
				.map(|c| LogEntry {
					command: c,
					term: self.term,
				})
				.collect(),
		});

		// broadcast the new log entries to all followers.
		let followers = shared.bonds().broadcast_raft::<M>(message);

		if !followers.is_empty() {
			let range = prev_pos.index().next()..=prev_pos.index() + count.into();

			tracing::trace!(
				followers = %FmtIter::<Short<_>, _>::new(followers),
				ix_range = ?range,
				group = %Short(shared.group_id()),
				network = %Short(shared.network_id()),
				"broadcasted {count} new log entries to",
			);
		}

		self.reset_heartbeat_timeout();
		Poll::Ready(())
	}

	/// Checks if the heartbeat timeout has elapsed without any new client command
	/// being published.
	fn poll_next_heartbeat<S: Storage<M::Command>>(
		&mut self,
		cx: &mut Context,
		shared: &Shared<S, M>,
	) -> Poll<()> {
		if self.heartbeat_timeout.as_mut().poll(cx).is_pending() {
			return Poll::Pending;
		}

		let prev_pos = shared.log.last();
		let heartbeat = AppendEntries::<M::Command> {
			term: self.term,
			leader: shared.local_id(),
			prev_log_position: prev_pos,
			entries: Vec::new(),
			leader_commit: shared.log.committed(),
		};

		shared
			.bonds()
			.broadcast_raft::<M>(Message::AppendEntries(heartbeat));

		self.reset_heartbeat_timeout();
		Poll::Ready(())
	}

	/// Called every time we send `AppendEntries` to followers.
	fn reset_heartbeat_timeout(&mut self) {
		let next_heartbeat = Instant::now() + self.heartbeat_interval;
		self.heartbeat_timeout.as_mut().reset(next_heartbeat.into());

		for waker in self.wakers.drain(..) {
			waker.wake();
		}
	}
}

#[derive(Debug)]
struct Committee {
	/// Map of current voters in the committee to their last acknowledged log
	/// index, which is used to track their replication progress and determine
	/// the commit index based on the highest log index that has been replicated
	/// to a quorum of voters.
	voters: HashMap<PeerId, Index>,

	/// Map of non-voting followers that have acknowledged our `AppendEntries`
	/// and are caught up with our log, and can be promoted to voters if needed.
	non_voters: HashMap<PeerId, Index>,

	/// The maximum number of voters that can be part of the voting committee at
	/// any given time. This is used to limit the size of the voting committee
	/// and ensure that we can achieve quorum with a reasonable number of
	/// voters.
	max_committee_size: usize,
}

impl Committee {
	/// Initializes a new voting committee at the beginning of a new leader term
	/// based on the peers that granted their vote to us in the election.
	///
	/// the list of voters always includes the leader itself.
	pub fn new<S: Storage<M::Command>, M: StateMachine>(
		voters: HashSet<PeerId>,
		shared: &Shared<S, M>,
	) -> Self {
		let last_log_index = shared.log.last().index();
		let voters = voters
			.into_iter()
			.map(|voter| (voter, last_log_index))
			.collect();

		Self {
			voters,
			non_voters: HashMap::new(),
			max_committee_size: 5,
		}
	}

	/// Records a vote from a follower that has acknowledged our `AppendEntries`
	/// message and is up to date with our log.
	///
	/// Followers that grant positive votes can be considered as part of the
	/// current voting committee.
	///
	/// Returns the index of the latest log entry that has been replicated to a
	/// quorum of voters in the current voting committee after recording this
	/// vote, which can be used by the leader to advance the commit index.
	pub fn record_vote(&mut self, follower: PeerId, log_index: Index) -> Index {
		// if the follower is already a voter, just update its last acknowledged log
		// index.
		if let Some(voter) = self.voters.get_mut(&follower) {
			*voter = log_index;
		} else if self.non_voters.insert(follower, log_index).is_none() {
			// new non-voting follower that acknowledged our `AppendEntries`, try to
			// promote it to a voter if we have room in the committee based on the
			// target committee size.
			self.try_backfill_voters();
		}

		// after recording this vote, check if we have reached a quorum with the
		// current committee and can advance the commit index.
		self.highest_quorum_index()
	}

	/// Removes voters from the committee that don't have active bonds with the
	/// leader. This is a best-effort cleanup mechanism to prevent the committee
	/// from being filled with voters that are not actively replicating log
	/// entries and are effectively offline, which would reduce the leader's
	/// ability to commit new log entries and make progress.
	pub fn remove_dead_voters<S: Storage<M::Command>, M: StateMachine>(
		&mut self,
		shared: &Shared<S, M>,
	) {
		let active_bonds = shared
			.bonds()
			.iter()
			.map(|bond| *bond.peer().id())
			.chain(once(shared.local_id()))
			.collect::<HashSet<_>>();

		self.voters.retain(|voter, _| active_bonds.contains(voter));
		self
			.non_voters
			.retain(|non_voter, _| active_bonds.contains(non_voter));

		self.try_backfill_voters();
	}

	/// Removes a follower from the voting committee that has casted a negative
	/// vote for our `AppendEntries` message.
	pub fn remove(&mut self, follower: PeerId) {
		self.voters.remove(&follower);
		self.non_voters.remove(&follower);
		self.try_backfill_voters();
	}

	/// Finds the highest log index that has been replicated to a quorum of voters
	/// in the current voting committee
	pub fn highest_quorum_index(&self) -> Index {
		let mut log_indices = self.voters.values().collect::<Vec<_>>();
		log_indices.sort_unstable();

		let quorum = (self.voters.len() / 2) + 1;
		*log_indices[log_indices.len() - quorum]
	}

	/// We want to keep the voting committee capped at `self.max_committee_size`
	/// voters to ensure we can achieve quorum with a reasonable latency, we also
	/// want to have an odd number of voters to avoid split votes.
	fn target_committee_size(&self) -> usize {
		let total_followers = self.voters.len() + self.non_voters.len();
		let capped = self.max_committee_size.min(total_followers);
		if capped <= 2 { capped } else { capped | 1 }
	}

	/// Attempts to backfill voters from the non-voters if there is room in the
	/// committee based on the target committee size.
	fn try_backfill_voters(&mut self) {
		// check if we should promote any non-voters to voters based on their log
		// progress and the current size of the voting committee.
		let target_committee_size = self.target_committee_size();
		if self.voters.len() < target_committee_size {
			// promote the most caught-up non-voters to fill the committee, but
			// ensure the resulting voter count is always odd to avoid split votes.
			let mut non_voters = self.non_voters.iter().collect::<Vec<_>>();
			let deficit = target_committee_size - self.voters.len();
			let available = non_voters.len().min(deficit);

			// if promoting `available` non-voters would result in an even voter
			// count, promote one fewer to keep it odd.
			let promote_count = if (self.voters.len() + available).is_multiple_of(2) {
				available.saturating_sub(1)
			} else {
				available
			};

			// sort non-voters by their log index in descending order to promote the
			// most caught-up ones first.
			non_voters.sort_by_key(|(_, log_index)| Reverse(*log_index));

			let promoted: Vec<_> = non_voters
				.into_iter()
				.take(promote_count)
				.map(|(id, idx)| (*id, *idx))
				.collect();

			for (non_voter, log_index) in promoted {
				self.non_voters.remove(&non_voter);
				self.voters.insert(non_voter, log_index);
			}
		}
	}
}
