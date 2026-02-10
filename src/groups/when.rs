use {crate::PeerId, tokio::sync::watch};

/// Awaits changes to the group's state.
#[derive(Debug)]
pub struct When {
	/// `PeerId` of the local node.
	local_id: PeerId,

	/// Observer for the current leader of the group.
	leader: watch::Sender<Option<PeerId>>,
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
}

/// Internal API
impl When {
	pub(crate) fn new(local_id: PeerId) -> Self {
		let leader = watch::Sender::new(None);
		Self { local_id, leader }
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

	/// Returns the current leader of the group.
	pub(super) fn current_leader(&self) -> Option<PeerId> {
		*self.leader.borrow()
	}
}
