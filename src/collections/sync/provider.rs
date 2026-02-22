use {
	super::{
		Config,
		FetchDataResponse,
		Snapshot,
		SnapshotRequest,
		SnapshotStateMachine,
		SnapshotSync,
		SnapshotSyncMessage,
		SyncInitCommand,
	},
	crate::{
		PeerId,
		collections::sync::SnapshotInfo,
		groups::*,
		primitives::Short,
	},
	chrono::Utc,
	core::{
		marker::PhantomData,
		pin::Pin,
		task::{Context, Poll},
		time::Duration,
	},
	std::collections::{HashMap, VecDeque, hash_map::Entry},
	tokio::{
		sync::{
			broadcast,
			mpsc::{UnboundedReceiver, UnboundedSender},
		},
		time::{Instant, Sleep, sleep},
	},
};

pub struct SnapshotSyncProvider<M: SnapshotStateMachine> {
	// must match on all instances of the collection on all peers in the group.
	config: Config,

	/// Translates a snapshot request to a state-machine-specific command that
	/// will be fed to the group for replication.
	sync_init_cmd: SyncInitCommand<M>,

	/// Receives snapshot requests from the state machine when they appear in the
	/// log.
	requests_rx: UnboundedReceiver<(SnapshotRequest, Cursor, M::Snapshot)>,

	/// The list of available snapshots that can be served to followers, indexed
	/// by their anchor position.
	available: HashMap<Cursor, AvailableSnapshot<M>>,
}

impl<M: SnapshotStateMachine> SnapshotSyncProvider<M> {
	pub(super) fn new(
		config: Config,
		sync_init_cmd: SyncInitCommand<M>,
		requests_rx: UnboundedReceiver<(SnapshotRequest, Cursor, M::Snapshot)>,
		_cx: &dyn SyncContext<SnapshotSync<M>>,
	) -> Self {
		Self {
			config,
			requests_rx,
			sync_init_cmd,
			available: HashMap::new(),
		}
	}
}

impl<M: SnapshotStateMachine> StateSyncProvider for SnapshotSyncProvider<M> {
	type Owner = SnapshotSync<M>;

	fn poll(
		&mut self,
		task_cx: &mut Context<'_>,
		sync_cx: &mut dyn SyncProviderContext<Self::Owner>,
	) -> Poll<()> {
		// process incoming snapshot requests
		if self.poll_pending_requests(task_cx, sync_cx).is_ready() {
			return Poll::Ready(());
		}

		// clean expired snapshots from the available list
		self.prune_expired_snapshots(task_cx);

		Poll::Pending
	}

	fn receive(
		&mut self,
		message: SnapshotSyncMessage<<M::Snapshot as Snapshot>::Item>,
		sender: PeerId,
		cx: &mut dyn SyncProviderContext<Self::Owner>,
	) -> Result<(), SnapshotSyncMessage<<M::Snapshot as Snapshot>::Item>> {
		match message {
			SnapshotSyncMessage::RequestSnapshot => {
				if !cx.is_leader() {
					tracing::debug!(
						from = %sender,
						group = %cx.group_id(),
						network = %cx.network_id(),
						"ignoring snapshot request on a non-leader node"
					);
					return Ok(());
				}

				// create a snapshot request and translate it to a state-machine's
				// specific command then feed it to the group for replication at a
				// later position in the log that will be seen by all peers in the
				// group.
				let request = (self.sync_init_cmd)(SnapshotRequest {
					requested_by: sender,
					requested_at: Utc::now(),
				});

				if cx.feed_command(request).is_err() {
					tracing::debug!(
						from = %sender,
						group = %cx.group_id(),
						network = %cx.network_id(),
						"failed to schedule snapshot request"
					);
				} else {
					tracing::trace!(
						by = %Short(sender),
						group = %cx.group_id(),
						network = %cx.network_id(),
						"snapshot sync requested"
					);
				}

				Ok(())
			}
			SnapshotSyncMessage::FetchDataRequest(request) => {
				// Look up the snapshot for the requested anchor
				if let Some(available) = self.available.get_mut(&request.anchor) {
					// Revive the snapshot TTL on each fetch
					available.revive(self.config.snapshot_ttl);

					if let Some(items) =
						available.snapshot.iter_range(request.range.clone())
					{
						let items: std::vec::Vec<_> = items.collect();
						cx.send_to(
							sender,
							SnapshotSyncMessage::FetchDataResponse(FetchDataResponse {
								anchor: request.anchor,
								offset: request.range.start,
								items,
							}),
						);
					} else {
						tracing::warn!(
							from = %Short(sender),
							anchor = %request.anchor,
							range = ?request.range,
							group = %cx.group_id(),
							network = %cx.network_id(),
							"snapshot range out of bounds"
						);
					}
				} else {
					tracing::debug!(
						from = %Short(sender),
						anchor = %request.anchor,
						group = %cx.group_id(),
						network = %cx.network_id(),
						"requested snapshot not available (expired or unknown)"
					);
				}

				Ok(())
			}
			other => Err(other),
		}
	}

	/// Snapshot-based sync does not need log entries for replay — committed
	/// entries are safe to prune since lagging followers will catch up via
	/// snapshots rather than log replay.
	fn safe_to_prune_prefix(
		&self,
		cx: &mut dyn SyncProviderContext<Self::Owner>,
	) -> Option<Index> {
		Some(cx.committed().index())
	}
}

impl<M: SnapshotStateMachine> SnapshotSyncProvider<M> {
	/// walks through the pending snapshot requests and notifies requesters of
	/// their availability.
	fn poll_pending_requests(
		&mut self,
		task_cx: &mut Context<'_>,
		sync_cx: &mut dyn SyncProviderContext<SnapshotSync<M>>,
	) -> Poll<()> {
		if !self.requests_rx.is_empty() {
			let mut requests = Vec::with_capacity(self.requests_rx.len());
			if self
				.requests_rx
				.poll_recv_many(task_cx, &mut requests, self.requests_rx.len())
				.is_ready()
			{
				for (request, position, snapshot) in requests {
					if request.requested_by == sync_cx.local_id() {
						// this is our own request, we can ignore it since we will
						// have the snapshot available locally as soon as it's
						// committed.
						continue;
					}

					let len = snapshot.len();
					match self.available.entry(position) {
						Entry::Occupied(mut existing) => {
							// extend the TTL of the existing snapshot if a new snapshot for
							// the same position is being served.
							existing.get_mut().revive(self.config.snapshot_ttl);
						}
						Entry::Vacant(place) => {
							// new snapshot for this position, add it to the available list.
							place.insert(AvailableSnapshot::new(
								snapshot,
								self.config.snapshot_ttl,
							));
						}
					}

					// notify the requester that the snapshot is available.
					sync_cx.send_to(
						request.requested_by,
						SnapshotInfo {
							anchor: position,
							len,
						}
						.into(),
					);
				}
			}
		}

		Poll::Pending
	}

	/// Removes snapshots that have not been active for longer than the configured
	/// TTL from the available snapshots list.
	fn prune_expired_snapshots(&mut self, cx: &mut Context<'_>) {
		let mut expired = Vec::new();
		for (pos, snapshot) in &mut self.available {
			if snapshot.poll_expired(cx).is_ready() {
				expired.push(*pos);
			}
		}

		for pos in expired {
			self.available.remove(&pos);
		}
	}
}

struct AvailableSnapshot<M: SnapshotStateMachine> {
	snapshot: M::Snapshot,
	expired: Pin<Box<Sleep>>,
}

impl<M: SnapshotStateMachine> AvailableSnapshot<M> {
	fn new(snapshot: M::Snapshot, ttl: Duration) -> Self {
		Self {
			snapshot,
			expired: Box::pin(sleep(ttl)),
		}
	}

	fn is_expired(&self) -> bool {
		self.expired.as_ref().is_elapsed()
	}

	fn revive(&mut self, ttl: Duration) {
		self.expired.as_mut().reset(Instant::now() + ttl);
	}

	fn poll_expired(&mut self, cx: &mut Context<'_>) -> Poll<()> {
		match self.expired.as_mut().poll(cx) {
			Poll::Ready(()) => Poll::Ready(()),
			Poll::Pending => Poll::Pending,
		}
	}
}
