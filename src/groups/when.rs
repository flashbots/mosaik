use {
	crate::{PeerId, groups::Index},
	tokio::sync::watch,
};

/// Awaits changes to the group's state.
#[derive(Debug)]
pub struct When {
	/// `PeerId` of the local node.
	local_id: PeerId,

	/// Observer for the current leader of the group.
	leader: watch::Sender<Option<PeerId>>,

	/// Observer for whether the local node is considered online.
	/// See `is_online` and `is_offline` for the definition of online and
	/// offline.
	online: watch::Sender<bool>,

	/// Observer for the committed index of the group's log.
	committed: watch::Sender<Index>,
}

/// Public API
impl When {
	/// Returns a future that resolves when a group leader is elected.
	/// Resolves immediately if a leader is already elected.
	pub fn leader_elected(
		&self,
	) -> impl Future<Output = PeerId> + Send + Sync + 'static {
		let mut leader = self.leader.subscribe();

		async move {
			leader.mark_changed();

			loop {
				let value = *leader.borrow_and_update();
				if let Some(leader) = value {
					return leader;
				}

				if leader.changed().await.is_err() {
					// if the watch channel is closed, consider no leader will be
					// elected and never resolve this future
					core::future::pending::<()>().await;
				}
			}
		}
	}

	/// Returns a future that resolves when the group leader changes.
	/// Resolves on next leader change; does not resolve immediately.
	pub fn leader_changed(
		&self,
	) -> impl Future<Output = PeerId> + Send + Sync + 'static {
		let mut leader = self.leader.subscribe();
		let current_leader = *leader.borrow();
		leader.mark_changed();

		async move {
			loop {
				if leader.changed().await.is_err() {
					// if the watch channel is closed, consider no leader will be
					// elected and never resolve this future
					core::future::pending::<()>().await;
				}

				let value = *leader.borrow_and_update();
				if let Some(new_leader) = value {
					if Some(new_leader) != current_leader {
						return new_leader;
					}
				}
			}
		}
	}

	/// Returns a future that resolves when the local node assumes leadership of
	/// the group.
	///
	/// Resolves immediately if the local node is already the leader.
	pub fn is_leader(&self) -> impl Future<Output = ()> + Send + Sync + 'static {
		let local_id = self.local_id;
		let mut leader = self.leader.subscribe();

		async move {
			leader.mark_changed();

			if leader.wait_for(|v| *v == Some(local_id)).await.is_err() {
				// if the watch channel is closed, consider the node not leader and
				// never resolve this future
				core::future::pending::<()>().await;
			}
		}
	}

	/// Returns a future that resolves when the local node becomes a follower in
	/// the group.
	///
	/// Resolves immediately if the local node is already a follower.
	pub fn is_follower(
		&self,
	) -> impl Future<Output = ()> + Send + Sync + 'static {
		let local_id = self.local_id;
		let mut leader = self.leader.subscribe();

		async move {
			leader.mark_changed();

			if leader.wait_for(|v| *v != Some(local_id)).await.is_err() {
				// if the watch channel is closed, consider the node not follower and
				// never resolve this future
				core::future::pending::<()>().await;
			}
		}
	}

	/// Returns a future that resolves when the local node is considered online
	/// and can be used to send commands to the group and query the state
	/// machine. Resolves immediately if the local node is already online.
	///
	/// A node is online when:
	/// - it is currently not in the middle of an election either as a candidate
	///   or a voter, and
	/// - It is currently the leader, or
	/// - It is currently a follower and is up to date with the current leader
	///   (i.e. it is not in the middle of catching up with the log or during
	///   elections).
	pub fn is_online(&self) -> impl Future<Output = ()> + Send + Sync + 'static {
		let mut online = self.online.subscribe();

		async move {
			if online.wait_for(|v| *v).await.is_err() {
				// if the watch channel is closed, consider the node not online and
				// never resolve this future
				core::future::pending::<()>().await;
			}
		}
	}

	/// Returns a future that resolves when the local node is considered offline
	/// and should not be used to send commands to the group or query the state
	/// machine. Resolves immediately if the local node is already offline.
	///
	/// A node is offline when it is not online, i.e. when:
	/// - It is currently a follower and is not up to date with the current leader
	/// - It is in the middle of an election (i.e. it is a candidate) or voting in
	///   an election (i.e. it is a follower that has voted for a candidate and is
	///   waiting for the election to complete).
	pub fn is_offline(&self) -> impl Future<Output = ()> + Send + Sync + 'static {
		let mut online = self.online.subscribe();

		async move {
			if online.wait_for(|v| !*v).await.is_err() {
				// if the watch channel is closed, consider the node not offline and
				// never resolve this future
				core::future::pending::<()>().await;
			}
		}
	}

	/// Returns a future that resolves when the committed index of the group's log
	/// advances.
	pub fn committed(
		&self,
	) -> impl Future<Output = Index> + Send + Sync + 'static {
		let mut committed = self.committed.subscribe();

		async move {
			if committed.changed().await.is_err() {
				// if the watch channel is closed, consider no new commits and never
				// resolve this future
				core::future::pending::<()>().await;
			}

			*committed.borrow_and_update()
		}
	}

	/// Returns a future that resolves when the committed index of the group's log
	/// advances to at least the given index.
	pub fn committed_up_to(
		&self,
		index: Index,
	) -> impl Future<Output = Index> + Send + Sync + 'static {
		let mut committed = self.committed.subscribe();

		async move {
			if let Ok(index) = committed.wait_for(|v| *v >= index).await {
				return *index;
			}

			// if the watch channel is closed, consider no new commits and never
			// resolve this future
			core::future::pending::<()>().await;
			unreachable!();
		}
	}
}

/// Internal API
impl When {
	pub(crate) fn new(local_id: PeerId) -> Self {
		let leader = watch::Sender::new(None);
		let online = watch::Sender::new(false);
		let committed = watch::Sender::new(0);
		Self {
			local_id,
			leader,
			online,
			committed,
		}
	}

	/// Called by `Consensus` when the group leader is updated.
	pub(super) fn update_leader(&self, new_leader: Option<PeerId>) {
		self.leader.send_if_modified(|current| {
			if *current == new_leader {
				false
			} else {
				*current = new_leader;
				true
			}
		});
	}

	/// Called by `Consensus` when the local node's online status changes.
	pub(super) fn set_online_status(&self, is_online: bool) {
		self.online.send_if_modified(|current| {
			let prev_value = *current;
			if prev_value == is_online {
				false
			} else {
				*current = is_online;
				true
			}
		});
	}

	/// Called by `Consensus` when the committed index of the group's log
	/// advances.
	pub(super) fn update_committed(&self, new_committed: Index) {
		self.committed.send_if_modified(|current| {
			if *current < new_committed {
				*current = new_committed;
				true
			} else {
				false
			}
		});
	}

	/// Returns the current leader of the group.
	pub(super) fn current_leader(&self) -> Option<PeerId> {
		*self.leader.borrow()
	}

	/// Returns the index of the latest committed log entry in the group.
	pub(super) fn current_committed(&self) -> Index {
		*self.committed.borrow()
	}
}
