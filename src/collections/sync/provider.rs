use {
	super::{
		Config,
		Snapshot,
		SnapshotStateMachine,
		SnapshotSync,
		SyncInitCommand,
		protocol::*,
	},
	crate::{
		PeerId,
		collections::sync::PendingRequest,
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

/// Long-running state sync provider that responds to `SnapshotRequest`,
/// `FetchDataRequest` and `FetchDataResponse` from `SnapshotSyncSession` to
/// serve snapshots and its data to lagging followers.
pub struct SnapshotSyncProvider<M: SnapshotStateMachine> {
	// must match on all instances of the collection on all peers in the group.
	config: Config,

	/// Translates a snapshot request to a state-machine-specific command that
	/// will be fed to the group for replication.
	sync_init_cmd: SyncInitCommand<M>,

	/// Receives snapshot requests from the state machine when they appear in the
	/// log.
	requests_rx: UnboundedReceiver<PendingRequest<M>>,

	/// The list of available snapshots that can be served to followers, indexed
	/// by their anchor position.
	available: HashMap<Cursor, AvailableSnapshot<M>>,
}

impl<M: SnapshotStateMachine> SnapshotSyncProvider<M> {
	pub(super) fn new(
		config: Config,
		sync_init_cmd: SyncInitCommand<M>,
		requests_rx: UnboundedReceiver<PendingRequest<M>>,
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

	/// Handles incoming messages from other peers related to snapshot sync.
	///
	/// If a message is not handled at the provider level and should be forwarded
	/// to the session, it is returned as an error.
	fn receive(
		&mut self,
		message: SnapshotSyncMessage<<M::Snapshot as Snapshot>::Item>,
		sender: PeerId,
		cx: &mut dyn SyncProviderContext<Self::Owner>,
	) -> Result<(), SnapshotSyncMessage<<M::Snapshot as Snapshot>::Item>> {
		match message {
			// A follower is starting a state catch-up session because it is lagging
			// behind the state of the group.
			SnapshotSyncMessage::RequestSnapshot => {
				self.on_snapshot_request(
					SnapshotRequest {
						requested_by: sender,
						requested_at: Utc::now(),
					},
					cx,
				);
				Ok(())
			}
			// A follower is requesting a specific range of items from a snapshot
			// anchored at a specific position.
			SnapshotSyncMessage::FetchDataRequest(request) => {
				self.on_fetch_data_request(&request, sender, cx);
				Ok(())
			}
			// forward to `SnapshotSyncSession`
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
		let mut received = false;
		while let Poll::Ready(Some(pending)) = self.requests_rx.poll_recv(task_cx) {
			received = true;
			if pending.request.requested_by == sync_cx.local_id()
				|| self.config.is_expired(&pending.request)
			{
				// ignore requests that were made by this node or are stale by the
				// time they are being processed.
				continue;
			}

			let len = pending.snapshot.len();
			match self.available.entry(pending.position) {
				Entry::Occupied(mut existing) => {
					// extend the TTL of the existing snapshot if a new snapshot for
					// the same position is being served.
					existing.get_mut().revive(self.config.snapshot_ttl);
				}
				Entry::Vacant(place) => {
					// new snapshot for this position, add it to the available list.
					place.insert(AvailableSnapshot::new(
						pending.snapshot,
						self.config.snapshot_ttl,
					));
				}
			}

			// notify the requester that the snapshot is available.
			sync_cx.send_to(
				pending.request.requested_by,
				SnapshotInfo {
					anchor: pending.position,
					items_count: len,
				}
				.into(),
			);

			tracing::trace!(
				to = %Short(pending.request.requested_by),
				anchor = %pending.position,
				items = len,
				group = %sync_cx.group_id(),
				network = %sync_cx.network_id(),
				"offering snapshot"
			);
		}

		if received {
			Poll::Ready(())
		} else {
			Poll::Pending
		}
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

	/// Handles incoming snapshot requests by translating them to
	/// state-machine-specific commands and feeding them to the group for
	/// replication.
	fn on_snapshot_request(
		&self,
		request: SnapshotRequest,
		cx: &mut dyn SyncProviderContext<SnapshotSync<M>>,
	) {
		let sender = request.requested_by;
		if !cx.is_leader() {
			tracing::debug!(
				from = %sender,
				group = %cx.group_id(),
				network = %cx.network_id(),
				"ignoring snapshot request on a non-leader node"
			);
			return;
		}

		// create a snapshot request and translate it to a state-machine's
		// specific command then feed it to the group for replication at a
		// later position in the log that will be seen by all peers in the
		// group.
		let request = (self.sync_init_cmd)(request);

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
	}

	/// Handles incoming `FetchDataRequest` messages by looking up the requested
	/// snapshot and sending the requested range of items back to the requester if
	/// the snapshot is still available.
	///
	/// Each request extends the TTL of the snapshot to ensure that it remains
	/// available for the duration of the sync process with lagging followers.
	fn on_fetch_data_request(
		&mut self,
		request: &FetchDataRequest,
		sender: PeerId,
		cx: &mut dyn SyncProviderContext<SnapshotSync<M>>,
	) {
		let Some(available) = self.available.get_mut(&request.anchor) else {
			tracing::debug!(
				from = %Short(sender),
				anchor = %request.anchor,
				group = %cx.group_id(),
				network = %cx.network_id(),
				"requested snapshot not available (expired or unknown)"
			);
			return;
		};

		// clamp the requested range to the configured max batch size
		let end = request
			.range
			.end
			.min(request.range.start + self.config.fetch_batch_size);
		let range = request.range.start..end;

		if range.is_empty() {
			return;
		}

		// Revive the snapshot TTL on each fetch
		available.revive(self.config.snapshot_ttl);

		let Some(items) = available.snapshot.iter_range(range) else {
			tracing::warn!(
				from = %Short(sender),
				anchor = %request.anchor,
				range = ?request.range,
				group = %cx.group_id(),
				network = %cx.network_id(),
				"snapshot range out of bounds"
			);
			return;
		};

		let items: std::vec::Vec<_> = items.collect();
		cx.send_to(
			sender,
			SnapshotSyncMessage::FetchDataResponse(FetchDataResponse {
				anchor: request.anchor,
				offset: request.range.start,
				items,
			}),
		);
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
