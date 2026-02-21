use {
	crate::{
		PeerId,
		discovery::{Catalog, PeerEntryVersion, SignedPeerEntry},
		groups::{
			Bond,
			Bonds,
			Error,
			Groups,
			StateMachine,
			Storage,
			When,
			bond::{BondEvent, HandshakeStart},
			config::GroupConfig,
			error::AlreadyBonded,
			raft::Raft,
			state::{WorkerBondCommand, WorkerRaftCommand, WorkerState},
		},
		network::{Cancelled, link::Link},
		primitives::{AsyncWorkQueue, Short},
	},
	core::{any::TypeId, future::poll_fn, pin::Pin},
	futures::{Stream, StreamExt, stream::SelectAll},
	im::ordmap::Entry,
	iroh::protocol::AcceptError,
	std::sync::Arc,
	tokio::sync::{
		mpsc::{UnboundedReceiver, unbounded_channel},
		oneshot,
	},
	tokio_stream::wrappers::UnboundedReceiverStream,
};

/// A long running worker loop that manages that internal state of a joined
/// group. There is one instance of this worker per group id.
///
/// This worker loop is responsible for:
/// - Discovering new peers on the network that are also members of this group
///   and establishing bond connections to them.
/// - Maintaining link-level bonds to all discovered peers in the group and
///   tracking their state.
pub struct Worker<S, M>
where
	S: Storage<M::Command>,
	M: StateMachine,
{
	/// The internal state of the worker loop that is shared between the main
	/// long-running task and the external world.
	state: Arc<WorkerState>,

	/// Receiver for commands sent from the external world to the worker loop.
	bonds_cmd_rx: UnboundedReceiver<WorkerBondCommand>,

	/// Channel for receiving commands for the group's raft consensus.
	/// This is distinct from `commands_rx` to keep the `WorkerState` free of
	/// generic type parameters.
	raft_cmd_rx: UnboundedReceiver<WorkerRaftCommand<M>>,

	/// Aggregated stream of all events emitted by active bonds in this group.
	bond_events: BondEventsStream,

	/// The raft consensus protocol instance that manages the replicated state
	/// machine and its underlying storage.
	raft: Raft<S, M>,

	/// Pending async work to be processed by the worker loop.
	work_queue: AsyncWorkQueue,

	/// The latest version of the local peer entry that was observed by this
	/// worker loop.
	///
	/// This is used to observe changes to the local peer entry and broadcast
	/// updates to all active bonds in the group over the `PeerEntryUpdate` bond
	/// message when changes are observed. This ensures that all bonded peers
	/// have the latest information about the local peer entry, at lower latency
	/// than the standard discovery propagation mechanism.
	last_local: PeerEntryVersion,
}

impl<S, M> Worker<S, M>
where
	S: Storage<M::Command>,
	M: StateMachine,
{
	/// This should be called exactly once for each joined group id to start the
	/// worker loop for that group. Any subsequent attempts to join the same group
	/// id should call `WorkerState::public_handle` to get a handle to the
	/// existing worker loop instead of spawning a new one.
	pub fn spawn(
		groups: &Groups,
		config: GroupConfig,
		storage: S,
		state_machine: M,
	) -> Arc<WorkerState> {
		let (bonds_cmd_tx, bonds_cmd_rx) = unbounded_channel();
		let (raft_cmd_tx, raft_cmd_rx) = unbounded_channel();

		let worker_state = Arc::new(WorkerState {
			config,
			bonds_cmd_tx,
			raft_cmd_tx: Box::new(raft_cmd_tx),
			global_config: Arc::clone(&groups.config),
			local: groups.local.clone(),
			discovery: groups.discovery.clone(),
			bonds: Bonds::default(),
			cancel: groups.local.termination().child_token(),
			when: When::new(groups.local.id()),
			types: (TypeId::of::<M>(), TypeId::of::<S>()),
		});

		let worker_instance = Self {
			raft_cmd_rx,
			bonds_cmd_rx,
			state: Arc::clone(&worker_state),
			bond_events: SelectAll::default(),
			work_queue: AsyncWorkQueue::default(),
			raft: Raft::new(Arc::clone(&worker_state), storage, state_machine),
			last_local: groups.discovery.me().update_version(),
		};

		tokio::spawn(worker_instance.run());

		worker_state
	}
}

impl<S, M> Worker<S, M>
where
	S: Storage<M::Command>,
	M: StateMachine,
{
	/// the entry point to start the worker loop for a joined group. This function
	/// runs indefinitely until the `cancel` token is triggered, at which point it
	/// should gracefully shut down all active bonds and terminate the loop.
	async fn run(mut self) {
		self.on_init();

		// trigger initial catalog scan for peers in the group
		let mut catalog = self.state.discovery.catalog_watch();
		catalog.mark_changed();

		loop {
			tokio::select! {
				// triggered when the group is terminating,
				// either due to network shutdown or explicit leave
				() = self.state.cancel.cancelled() => {
					self.on_terminated();
					break;
				}

				// polls pending async work tasks and drives their execution
				_ = self.work_queue.next() => { }

				// polls the consensus protocol and drives its execution
				() = poll_fn(|cx| self.raft.poll(cx)) => { }

				// Triggered when there are changes to the discovery catalog
				_ = catalog.changed() => {
					let catalog = catalog.borrow_and_update().clone();
					self.on_catalog_update(catalog);
				}

				// Triggered when any peer bond emits an event
				Some((event, peer_id)) = self.bond_events.next() => {
					self.on_bond_event(event, peer_id);
				}

				// handles external commands sent to the worker loop
				Some(command) = self.bonds_cmd_rx.recv() => {
					self.on_worker_command(command);
				}

				// handles raft consensus commands sent to the worker loop
				Some(command) = self.raft_cmd_rx.recv() => {
					self.on_raft_command(command);
				}
			}
		}
	}
}

// event handlers
impl<S, M> Worker<S, M>
where
	S: Storage<M::Command>,
	M: StateMachine,
{
	/// When the group worker loop instance is first created and initialized.
	///
	/// Updates the discovery catalog to include this group in the local peer
	/// entry so that other peers can discover that we are a member and attempt to
	/// bond with us when they join the same group.
	fn on_init(&self) {
		let group_id = *self.state.group_id();
		self
			.state
			.discovery
			.update_local_entry(move |entry| entry.add_groups(group_id));

		tracing::info!(
			group = %Short(group_id),
			network = %self.state.network_id(),
			"joining",
		);
	}

	/// When the group instance is terminated, this happens when the network is
	/// shutting down or when the group is being explicitly left.
	fn on_terminated(&self) {
		tracing::debug!(
			group = %Short(self.state.group_id()),
			network = %self.state.network_id(),
			"leaving group",
		);
	}

	/// Triggered when the discovery subsystem signals that the catalog has new
	/// information. Here we look for new peers that are members of this group
	/// but have no active bond yet, and create bonds to them.
	#[expect(clippy::needless_pass_by_value)]
	fn on_catalog_update(&mut self, snapshot: Catalog) {
		// Find new peers that have joined the group but are not yet tracked
		// by this worker loop.
		let new_peers_in_group = snapshot
			.signed_peers()
			.filter(|peer| peer.groups().contains(self.state.group_id()));

		for peer in new_peers_in_group {
			self.create_bond(peer.clone());
		}

		// Check if our local peer entry has been updated. If so, broadcast
		// the changes to all active bonds in the group.
		let me = self.state.discovery.me();
		if me.update_version() > self.last_local {
			// our local peer entry has been updated, broadcast the changes
			// to all active bonds in the group.
			self.last_local = me.update_version();
			self.state.bonds.notify_local_info_update(&me);
		}
	}

	/// Handles bond events from active bonds in the group.
	fn on_bond_event(&mut self, event: BondEvent, peer_id: PeerId) {
		match event {
			// a connected peer has disconnected
			BondEvent::Terminated(reason) => {
				// remove the bond from the active list
				self.state.bonds.update_with(|active| {
					if let Some(bond) = active.remove(&peer_id)
						&& reason != AlreadyBonded
					{
						tracing::debug!(
							id = %Short(bond.id()),
							group = %Short(self.state.group_id()),
							peer = %Short(peer_id),
							network = %self.state.network_id(),
							reason = %reason,
							"bond terminated",
						);
					}
				});
			}

			// a connected peer has formed a new bond with some other peer.
			BondEvent::Connected => {
				self.on_bond_formed(peer_id);
			}

			// a connected peer has sent us a raft-protocol message
			BondEvent::Raft(message) => {
				self.raft.receive_protocol_message(message, peer_id);
			}
		}
	}

	/// Invoked when the a new bond is successfully formed with a new peer in the
	/// group.
	///
	/// This will notify all other bonded peers in the group about the new bond so
	/// they can attempt to create bonds with the new peer as well. This helps to
	/// propagate connectivity information about peers in the group faster than
	/// waiting for discovery updates to propagate through the network.
	fn on_bond_formed(&self, peer_id: PeerId) {
		let Some(bond) = self.state.bonds.get(&peer_id) else {
			// bond should exist for connected peer, peer dropped before we could
			// process the event
			return;
		};

		tracing::debug!(
			id = %Short(bond.id()),
			peer = %Short(peer_id),
			group = %Short(self.state.group_id()),
			network = %self.state.network_id(),
			"bond established",
		);

		let catalog = self.state.discovery.catalog();
		let Some(peer_entry) = catalog.get_signed(&peer_id).cloned() else {
			tracing::warn!(
				network = %self.state.network_id(),
				peer = %Short(peer_id),
				group = %Short(self.state.group_id()),
				"peer entry not found in catalog after bond formed",
			);
			return;
		};

		// Notify all bonded peers that a new bond has been formed with this peer.
		self.state.bonds.notify_bond_formed(&peer_entry);
	}

	/// Handles incoming external commands sent to the worker loop.
	fn on_worker_command(&mut self, command: WorkerBondCommand) {
		match command {
			// Begins the process of accepting an incoming connection for this
			WorkerBondCommand::Accept(link, peer, handshake, result_tx) => {
				self.accept_bond(*link, peer, handshake, result_tx);
			}
			// Attempts to create a new bond connection to the specified peer.
			WorkerBondCommand::Connect(peer_entry) => {
				self.create_bond(peer_entry);
			}
			// Subscribes to bond events from a newly created bond
			WorkerBondCommand::SubscribeToBond(events_rx, peer_id) => {
				self.bond_events.push(Box::pin(
					UnboundedReceiverStream::new(events_rx)
						.map(move |event| (event, peer_id)),
				));
			}
		}
	}

	/// Handles incoming commands for the raft consensus protocol.
	fn on_raft_command(&mut self, command: WorkerRaftCommand<M>) {
		match command {
			WorkerRaftCommand::Feed(cmd, result_tx) => {
				let cmd_fut = self.raft.feed(cmd);
				self.work_queue.enqueue(async move {
					let result = cmd_fut.await;
					let _ = result_tx.send(result);
				});
			}
			WorkerRaftCommand::Query(query, consistency, result_tx) => {
				let query_fut = self.raft.query(query, consistency);
				self.work_queue.enqueue(async move {
					let result = query_fut.await;
					let _ = result_tx.send(result);
				});
			}
		}
	}
}

// Bonds management
impl<S, M> Worker<S, M>
where
	S: Storage<M::Command>,
	M: StateMachine,
{
	/// Initiates the process of creating a new bond connection to a remote
	/// peer in the group.
	///
	/// This happens in response to discovering a new peer in the group via
	/// the discovery catalog. This method is called only for peers that are
	/// already known in the discovery catalog.
	fn create_bond(&self, peer: SignedPeerEntry) {
		if *peer.id() == self.state.local_id() {
			// don't bond with ourselves
			return;
		}

		if self.state.bonds.contains_peer(peer.id()) {
			// there's already an active bond to this peer
			return;
		}

		// initiate a new bond connection with this peer.
		let peer_id = *peer.id();
		let state = Arc::clone(&self.state);
		let fut = async move {
			match Bond::create(Arc::clone(&state), peer).await {
				Ok((handle, events)) => {
					state.bonds.update_with(|active| {
						match active.entry(peer_id) {
							Entry::Vacant(place) => {
								// subscribe to bond events
								if state
									.bonds_cmd_tx
									.send(WorkerBondCommand::SubscribeToBond(events, peer_id))
									.is_ok()
								{
									// keep track of the bond handle to control it
									place.insert(handle);
								}
							}
							Entry::Occupied(_) => {
								// a bond with this peer was created in the meantime
								// terminate the redundant connection.
								tokio::spawn(handle.close(AlreadyBonded));
							}
						}
					});
				}
				Err(reason) => {
					if !matches!(reason, Error::AlreadyBonded(_)) {
						tracing::trace!(
							error = %reason,
							network = %state.local.network_id(),
							peer = %Short(peer_id),
							group = %Short(state.group_id()),
							"bonding failed",
						);
					}
				}
			}
		};

		self.work_queue.enqueue(fut);
	}

	/// Given an incoming link and decoded handshake, begins the process of
	/// accepting the bond connection for this group. See [`Handle::accept`].
	fn accept_bond(
		&self,
		link: Link<Groups>,
		peer: SignedPeerEntry,
		handshake: HandshakeStart,
		result_tx: oneshot::Sender<Result<(), AcceptError>>,
	) {
		let peer_id = link.remote_id();
		assert_eq!(peer.id(), &peer_id);

		if self.state.bonds.contains_peer(&peer_id) {
			// there's already an active bond to this peer
			tokio::spawn(link.close(AlreadyBonded));
			let _ = result_tx.send(Err(AcceptError::from_err(AlreadyBonded)));
			return;
		}

		let state = Arc::clone(&self.state);
		let fut = async move {
			match Bond::accept(Arc::clone(&state), link, peer, handshake).await {
				Ok((handle, events)) => {
					state.bonds.update_with(|active| {
						match active.entry(peer_id) {
							Entry::Vacant(place) => {
								// subscribe to bond events
								if state
									.bonds_cmd_tx
									.send(WorkerBondCommand::SubscribeToBond(events, peer_id))
									.is_ok()
								{
									// keep track of the bond handle to control it
									place.insert(handle);
									let _ = result_tx.send(Ok(()));
								} else {
									let _ = result_tx.send(Err(AcceptError::from_err(Cancelled)));
									tokio::spawn(handle.close(Cancelled));
								}
							}
							Entry::Occupied(_) => {
								// a bond with this peer was created in the meantime
								tokio::spawn(handle.close(AlreadyBonded));
								let _ =
									result_tx.send(Err(AcceptError::from_err(AlreadyBonded)));
							}
						}
					});
				}
				Err(reason) => {
					let _ = result_tx.send(Err(AcceptError::from_err(reason)));
				}
			}
		};

		self.work_queue.enqueue(fut);
	}
}

/// Aggregated stream of bond events from all active bonds in the group.
type BondEventsStream = SelectAll<
	Pin<Box<dyn Stream<Item = (BondEvent, PeerId)> + Send + Sync + 'static>>,
>;
