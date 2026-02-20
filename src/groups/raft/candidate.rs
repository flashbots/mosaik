use {
	super::protocol::RequestVote,
	crate::{
		PeerId,
		groups::{
			StateMachine,
			Storage,
			Term,
			raft::{
				Message,
				leader::Leader,
				protocol::Vote,
				role::{Role, RoleHandlerError},
				shared::Shared,
			},
		},
		primitives::Short,
	},
	core::{
		iter::once,
		marker::PhantomData,
		ops::ControlFlow,
		pin::Pin,
		task::{Context, Poll},
	},
	std::collections::HashSet,
	tokio::time::{Sleep, sleep},
};

/// Internal state for the candidate role that is currently running elections
/// for its leadership candidacy.
#[derive(Debug)]
pub struct Candidate<M: StateMachine> {
	/// The current term for this node.
	term: Term,

	/// The set of peers from which votes have been requested.
	requested_from: HashSet<PeerId>,

	/// The set of peers that have granted their vote.
	granted: HashSet<PeerId>,

	/// The set of peers that have abstained from voting.
	abstained: HashSet<PeerId>,

	/// The set of peers that have explicitly denied their vote.
	denied: HashSet<PeerId>,

	/// The election timeout for the current election round. If this timeout
	/// elapses without reaching a quorum, a new election round will be started.
	election_timeout: Pin<Box<Sleep>>,

	/// Wakers for tasks that are waiting for the election result (e.g., to
	/// become leader or step down to follower). These tasks will be woken up
	/// when a quorum is reached or when stepping down to follower state.
	wakers: Vec<std::task::Waker>,

	#[doc(hidden)]
	_phantom: PhantomData<M>,
}

impl<M: StateMachine> Candidate<M> {
	/// Creates a new candidate role for the specified term and starts the
	/// election process by sending `RequestVote` messages to all bonded peers in
	/// the group.
	pub fn new<S: Storage<M::Command>>(
		term: Term,
		shared: &mut Shared<S, M>,
	) -> Self {
		assert_ne!(
			term,
			Term::zero(),
			"Candidate role should be at least in term 1"
		);

		// nodes in candidate state are always considered offline
		shared.set_offline();

		// make sure that we can vote in this term by checking if we have not
		// already voted for another candidate in this term.
		let mut term = term;
		loop {
			if shared.can_vote(term, shared.local_id()) {
				break;
			}
			term = term.next();
		}

		let election_timeout = shared.consensus().election_timeout();
		let election_timeout = Box::pin(sleep(election_timeout));

		let candidate = shared.local_id();
		let log_position = shared.storage.last();

		let request = RequestVote {
			term,
			candidate,
			log_position,
		};

		tracing::debug!(
			term = %term,
			log = %log_position,
			group = %Short(shared.group_id()),
			network = %Short(shared.network_id()),
			"starting new leader elections",
		);

		// Broadcast the `RequestVote` message to all bonded peers in the group.
		let requested_from = shared
			.bonds()
			.broadcast_raft::<M>(&Message::RequestVote(request));

		let requested_from =
			requested_from.into_iter().chain(once(candidate)).collect();

		// We start with granting our own vote to ourselves.
		let votes_granted = once(candidate).collect();
		shared.save_vote(term, candidate);

		Self {
			term,
			requested_from,
			granted: votes_granted,
			election_timeout,
			wakers: Vec::new(),
			abstained: HashSet::new(),
			denied: HashSet::new(),
			_phantom: PhantomData,
		}
	}
}

impl<M: StateMachine> Candidate<M> {
	/// As a candidate, we start elections and wait for votes from other nodes or
	/// `AppendEntries` from a leader. If no quorum is reached within the election
	/// timeout, we start a new election.
	///
	/// Returns `Poll::Ready(ControlFlow::Break(new_role))` if the role should
	/// transition to a new state (e.g., follower or leader) or `Poll::Pending`
	/// if it should continue waiting in the candidate state.
	pub fn poll_next_tick<S: Storage<M::Command>>(
		&mut self,
		cx: &mut Context,
		shared: &mut Shared<S, M>,
	) -> Poll<ControlFlow<Role<M>>> {
		if self.quorum_reached() {
			tracing::debug!(
				term = %self.term(),
				group = %Short(shared.group_id()),
				network = %Short(shared.network_id()),
				votes = self.format_vote_counts(),
				"quorum reached, becoming leader",
			);

			return Poll::Ready(ControlFlow::Break(
				Leader::new(self.term, self.granted.clone(), shared).into(),
			));
		}

		if self.election_timeout.as_mut().poll(cx).is_ready() {
			// election timeout elapsed without reaching a quorum, so we start a new
			// election by incrementing the term and broadcasting new RequestVote
			// message to all bonded peers in the group.
			tracing::debug!(
				term = %self.term(),
				group = %Short(shared.group_id()),
				network = %Short(shared.network_id()),
				committee_size = self.requested_from.len(),
				votes = self.format_vote_counts(),
				"quorum not reached",
			);

			return Poll::Ready(ControlFlow::Break(
				Self::new(self.term.next(), shared).into(),
			));
		}

		// register the waker for this task so that it can be woken up when we
		// receive enough votes to become leader.
		self.wakers.push(cx.waker().clone());

		Poll::Pending
	}

	/// The current term of this elections round.
	pub const fn term(&self) -> Term {
		self.term
	}

	/// When in a candidate state, we only expect to receive `RequestVoteResponse`
	/// messages from other nodes.
	///
	/// Returns Ok(()) if the message was handled by the candidate, or
	/// Err(message) if the message was unexpected and was not handled by this
	/// role.
	pub fn receive_protocol_message<S: Storage<M::Command>>(
		&mut self,
		message: Message<M>,
		sender: PeerId,
		shared: &Shared<S, M>,
	) -> Result<(), RoleHandlerError<M>> {
		let response = match message {
			// Raft paper 5.2: A candidate continues in the candidate state until it
			// either wins the election and becomes leader, or discovers that another
			// server has become leader or has a higher term, in which case it
			// immediately returns to follower state.
			Message::RequestVoteResponse(response) => response,

			// Raft paper 5.2: While waiting for votes, a candidate may receive an
			// AppendEntries RPC from another server claiming to be leader. If the
			// leader's term (included in its RPC) is at least as large as the
			// candidate's current term, then the candidate recognizes the leader as
			// legitimate and returns to follower state.
			Message::AppendEntries(request) => {
				return Err(RoleHandlerError::<M>::StepDown(request));
			}
			other => return Err(RoleHandlerError::<M>::Unexpected(other)),
		};

		if !self.requested_from.contains(&sender) {
			tracing::warn!(
				from = %Short(sender),
				term = %response.term,
				group = %Short(shared.group_id()),
				network = %Short(shared.network_id()),
				"ignoring unsolicited vote",
			);
			return Ok(());
		}

		match response.vote {
			Vote::Granted => {
				// If the peer granted its vote, we add it to the set of votes granted
				// to us. We will check if we have reached a quorum after processing the
				// vote and wake up any waiting
				self.granted.insert(sender);

				tracing::debug!(
					peer = %Short(sender),
					term = %response.term,
					group = %Short(shared.group_id()),
					network = %Short(shared.network_id()),
					votes = self.format_vote_counts(),
					"vote granted by",
				);
			}
			Vote::Abstained => {
				// If the peer abstained from voting, we don't count it towards our
				// granted votes, but we also don't count it against us. We will still
				// need to reach a quorum of granted votes from the remaining peers to
				// win the election, but this peer won't be part of the quorum
				// denominator until it catches up with the log and can vote again.
				self.abstained.insert(sender);

				tracing::debug!(
					peer = %Short(sender),
					term = %response.term,
					group = %Short(shared.group_id()),
					network = %Short(shared.network_id()),
					votes = self.format_vote_counts(),
					"vote abstained by",
				);
			}
			Vote::Denied => {
				// peer explicitly denied its vote, we add it to the set of denied
				// votes. This will count against us when calculating if we have reached
				// a quorum, but we will still wait for the
				self.denied.insert(sender);

				tracing::debug!(
					peer = %Short(sender),
					term = %response.term,
					group = %Short(shared.group_id()),
					network = %Short(shared.network_id()),
					votes = self.format_vote_counts(),
					"vote denied by",
				);
			}
		}

		if self.quorum_reached() || self.elections_lost() {
			// we know the outcome of the election, so we can wake up any waiting
			// tasks to transition to a new role.
			for waker in self.wakers.drain(..) {
				waker.wake();
			}
		}

		Ok(())
	}

	/// Checks if we have received votes from a quorum of peers to win the
	/// leader election.
	fn quorum_reached(&self) -> bool {
		self.granted.len() >= self.quorum()
	}

	/// Checks if we have received enough denied votes to lose the leader
	/// election.
	fn elections_lost(&self) -> bool {
		self.denied.len() >= self.quorum()
	}

	fn format_vote_counts(&self) -> String {
		// we are always a voter in our own elections, so we will never divide by
		// zero here,
		let percent = (self.granted.len() * 100)
			.checked_div(self.quorum())
			.expect("we always vote for ourselves as a candidate");

		format!(
			"[{}+/{}-/{}?/n={},q={}/{:.1}%]",
			self.granted.len(),
			self.denied.len(),
			self.abstained.len(),
			self.requested_from.len(),
			self.quorum(),
			percent
		)
	}

	/// The number of votes required to reach a quorum based on the number of
	/// votes requested and the number of abstained votes.
	fn quorum(&self) -> usize {
		let voters_count = self
			.requested_from
			.len()
			.saturating_sub(self.abstained.len());
		(voters_count / 2) + 1
	}
}
