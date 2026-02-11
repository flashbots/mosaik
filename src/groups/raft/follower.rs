use {
	crate::{
		PeerId,
		groups::{
			log::{StateMachine, Storage, Term},
			raft::{
				Message,
				candidate::Candidate,
				protocol::{AppendEntries, AppendEntriesResponse, Vote},
				role::Role,
				shared::Shared,
			},
		},
		primitives::Short,
	},
	core::{
		marker::PhantomData,
		ops::ControlFlow,
		pin::Pin,
		task::{Context, Poll},
	},
	std::time::Instant,
	tokio::time::{Sleep, sleep},
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
			election_timeout: Box::pin(sleep(election_timeout)),
			_marker: PhantomData,
		}
	}

	/// Returns the current term of this follower.
	pub const fn term(&self) -> Term {
		self.term
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
		if self.election_timeout.as_mut().poll(cx).is_ready() {
			// election timeout elapsed without hearing from a leader, so we start
			// a new election by transitioning to candidate state
			return Poll::Ready(ControlFlow::Break(
				Candidate::new(self.term + 1, shared).into(),
			));
		}

		Poll::Pending
	}

	/// In follower role there is only one message type that we expect to
	/// receive and handle at the role-specific level: `AppendEntries` from a
	/// leader. All other message types (e.g., `RequestVote` from candidates) are
	/// handled at the shared level since they can be received in any role and do
	/// not require any role-specific state to be processed.
	///
	/// Upon receiving a valid `AppendEntries` message from a leader, we reset the
	/// election timeout and update the current leader for this follower.
	///
	/// If our local log term and index are behind the ones in the `AppendEntries`
	/// message, this follower will transition into non-voting "catch-up" mode
	/// where it will attempt to download the missing log entries from other
	/// members of the group before it can confirm the `AppendEntries` message and
	/// update its log to match the leader's log. `AppendEntries` messages
	/// received while in catch-up are confirmed with `Abstained`
	/// `AppendEntriesResponse`.
	pub fn receive_protocol_message<S: Storage<M::Command>>(
		&mut self,
		message: Message<M::Command>,
		sender: PeerId,
		shared: &mut Shared<S, M>,
	) {
		let Message::AppendEntries(request) = message else {
			tracing::warn!(
				term = self.term(),
				sender = %Short(sender),
				group = %Short(shared.group_id()),
				network = %Short(shared.network_id()),
				"unexpected message: only AppendEntries is expected in follower state",
			);
			return;
		};

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
			self.accept_in_sync_entries(request, sender, shared);
			shared.set_online();
		} else {
			// Log is inconsistent - we are missing entries before `prev_log_index`.
			// Buffer the incoming entries and enter catch-up mode to fetch the gap
			// from peers.
			shared.set_offline();
			todo!("implement follower catch-up")
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
		if request.entries.is_empty() {
			// this is a heartbeat message from the leader with no new entries,
			// nothing to append and no need to respond to the leader with
			// `AppendEntriesResponse` since the leader doesn't need confirmation of
			// the log state from empty heartbeats.
			return;
		}

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

		// advance local commit index to the minimum of the leader's commit index
		// and the index of our last log entry, which ensures we only commit
		// entries that are both replicated to a majority
		let (_, last_log_index) = shared.log.last();
		let committed = request.leader_commit.min(last_log_index);
		shared.log.commit_up_to(committed);

		tracing::debug!(
			committed = committed,
			length = last_log_index,
			term = self.term(),
			group = %Short(shared.group_id()),
			network = %Short(shared.network_id()),
			"follower log"
		);

		shared.bonds().send_raft_message_to::<M>(
			Message::AppendEntriesResponse(AppendEntriesResponse {
				term: self.term(),
				vote: Vote::Granted,
				last_log_index,
			}),
			sender,
		);
	}
}
