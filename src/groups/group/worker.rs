use {
	crate::{
		GroupKey,
		Groups,
		NetworkId,
		PeerId,
		discovery::{Catalog, Discovery, PeerEntry},
		groups::{
			AlreadyBonded,
			Config,
			Error,
			InvalidAuth,
			accept::{HandshakeEnd, HandshakeStart},
			group::bond::{Bond, BondEvent},
		},
		network::{
			CloseReason,
			LocalNode,
			UnexpectedClose,
			UnknownPeer,
			link::Link,
		},
		primitives::Short,
	},
	core::pin::Pin,
	dashmap::{DashMap, Entry},
	futures::{
		Stream,
		StreamExt,
		stream::{FuturesUnordered, SelectAll},
	},
	iroh::{endpoint::ApplicationClose, protocol::AcceptError},
	std::sync::Arc,
	tokio::sync::{
		mpsc,
		mpsc::{UnboundedReceiver, UnboundedSender},
		oneshot,
	},
	tokio_stream::wrappers::UnboundedReceiverStream,
	tokio_util::sync::CancellationToken,
};

/// Internal handle for interacting with the background worker loop managing an
/// active group instance.
pub struct Handle {
	/// The group key associated with this group.
	key: GroupKey,

	/// The network id this group belongs to.
	network_id: NetworkId,

	/// Channel for sending commands to the worker loop.
	commands_tx: UnboundedSender<Command>,
}

/// Internal Read-Only API
impl Handle {
	/// Returns the group key associated with this group.
	pub const fn key(&self) -> &GroupKey {
		&self.key
	}

	/// Returns the network id this group belongs to.
	pub fn network_id(&self) -> &NetworkId {
		&self.network_id
	}
}

/// Internal API
impl Handle {
	/// Accepts an incoming bond connection for this group.
	///
	/// This is called by the group's protocol handler when a new connection
	/// is established  in [`Listener::accept`].
	///
	/// By the time this method is called:
	/// - The network id has already been verified to match the local node's
	///   network id.
	/// - The group id has already been verified to match this group's id.
	/// - The authentication proof has not been verified yet.
	/// - The presence of the remote peer in the local discovery catalog is not
	///   guaranteed.
	pub async fn accept(
		&self,
		link: Link<Groups>,
		handshake: HandshakeStart,
	) -> Result<(), AcceptError> {
		let (result_tx, result_rx) = oneshot::channel();
		let command = Command::Accept(link, handshake, result_tx);

		// handoff the accept process to the background worker loop
		self
			.commands_tx
			.send(command)
			.map_err(AcceptError::from_err)?;

		// wait for the worker loop to process the accept request
		result_rx.await.map_err(AcceptError::from_err)?
	}
}

pub struct WorkerLoop {
	config: Arc<Config>,
	handle: Arc<Handle>,
	local: LocalNode,
	discovery: Discovery,
	events: BondEventsStream,
	cancel: CancellationToken,
	active: Arc<DashMap<PeerId, Bond>>,
	commands_tx: UnboundedSender<Command>,
	commands_rx: UnboundedReceiver<Command>,
	pending_work: AsyncWorkQueue,
}

impl WorkerLoop {
	pub(super) fn spawn(
		key: GroupKey,
		local: &LocalNode,
		discovery: Discovery,
		config: Arc<Config>,
	) -> Arc<Handle> {
		let active = Arc::new(DashMap::new());
		let cancel = local.termination().child_token();
		let (commands_tx, commands_rx) = mpsc::unbounded_channel();

		let handle = Arc::new(Handle {
			key,
			network_id: *local.network_id(),
			commands_tx: commands_tx.clone(),
		});

		let worker = Self {
			config,
			discovery,
			cancel,
			active,
			commands_tx,
			commands_rx,
			local: local.clone(),
			handle: Arc::clone(&handle),
			pending_work: AsyncWorkQueue::new(),
			events: BondEventsStream::new(),
		};

		tokio::spawn(worker.run());

		handle
	}
}

impl WorkerLoop {
	async fn run(mut self) -> Result<(), Error> {
		// trigger initial catalog scan for peers in the group
		let mut catalog = self.discovery.catalog_watch();
		catalog.mark_changed();

		loop {
			tokio::select! {
				() = self.cancel.cancelled() => {
					self.on_terminated();
					break;
				}

				// polls pending async work tasks
				_ = self.pending_work.next() => {
					// a pending task has completed
				}

				_ = catalog.changed() => {
					let catalog = catalog.borrow_and_update().clone();
					self.on_catalog_update(catalog);
				}

				// handles events from all active bonds in the group
				Some((event, peer_id)) = self.events.next() => {
					self.on_bond_event(event, peer_id);
				}

				// handles external commands sent to the worker loop
				Some(command) = self.commands_rx.recv() => {
					self.on_external_command(command);
				}
			}
		}
		Ok(())
	}

	/// Handles incoming external commands sent to the worker loop.
	fn on_external_command(&mut self, command: Command) {
		match command {
			// Begins the process of accepting an incoming connection for this
			Command::Accept(link, handshake, result_tx) => {
				self
					.pending_work
					.push(Box::pin(self.accept_bond(link, handshake, result_tx)));
			}
			// Subscribes to bond events from a newly created bond
			Command::SubscribeToBond(events_rx, peer_id) => {
				self.events.push(Box::pin(
					UnboundedReceiverStream::new(events_rx)
						.map(move |event| (event, peer_id)),
				));
			}
			// Enqueues arbitrary asynchronous work to be processed by the worker
			// loop.
			Command::EnqueueWork(fut) => {
				self.pending_work.push(fut);
			}

			// Initiates creating a new bond connection to a remote peer in the group.
			Command::CreateBond(peer) => {
				self.pending_work.push(Box::pin(self.create_bond(peer)));
			}
		}
	}

	/// Handles bond events from active bonds in the group.
	fn on_bond_event(&self, event: BondEvent, peer_id: PeerId) {
		match event {
			BondEvent::Terminated => {
				self.active.remove(&peer_id);
				tracing::info!(
					network = %self.local.network_id(),
					peer = %Short(peer_id),
					group = %Short(self.handle.key.id()),
					"bond connection terminated",
				);
			}
		}
	}

	fn on_terminated(&self) {}

	fn on_catalog_update(&mut self, snapshot: Catalog) {
		// Find new peers that have joined the group but are not yet tracked
		// by this worker loop.
		let new_peers_in_group = snapshot.peers().filter(|peer| {
			peer.groups().contains(self.handle.key.id())
				&& !self.active.contains_key(peer.id())
		});

		for peer in new_peers_in_group {
			self
				.pending_work
				.push(Box::pin(self.create_bond(peer.clone())));
		}
	}

	/// Initiates the process of creating a new bond connection to a remote
	/// peer in the group.
	///
	/// This happens in response to discovering a new peer in the group via
	/// the discovery catalog. This method is called only for peers that are
	/// already known in the discovery catalog.
	fn create_bond(
		&self,
		peer: PeerEntry,
	) -> impl Future<Output = ()> + Send + Sync + 'static {
		let config = Arc::clone(&self.config);
		let cancel = self.cancel.clone();
		let local_id = self.local.id();
		let active: Arc<DashMap<PeerId, Bond>> = Arc::clone(&self.active);
		let network_id = *self.handle.network_id();
		let group_key = self.handle.key().clone();
		let commands_tx = self.commands_tx.clone();
		let discovery = self.discovery.clone();

		let connect_fut = self.local.connect_with_cancel::<Groups>(
			peer.address().clone(),
			self.cancel.child_token(),
		);

		async move {
			tracing::trace!(
				network = %network_id,
				peer = %Short(peer.id()),
				group = %Short(group_key.id()),
				"initiating outgoing bond connection",
			);

			// if the peer is already bonded, skip creating a new bond
			if active.contains_key(peer.id()) {
				// already bonded
				tracing::trace!(
					network = %network_id,
					peer = %Short(peer.id()),
					group = %Short(group_key.id()),
					"skipping outgoing bond: already bonded",
				);
				return;
			}

			// attempt to establish a new link to the remote peer
			let Ok(mut link) = connect_fut.await.inspect_err(|e| {
				tracing::debug!(
					network = %network_id,
					peer = %Short(peer.id()),
					group = %Short(group_key.id()),
					error = %e,
					"failed to connect to peer for bonding",
				);
			}) else {
				return;
			};

			// prepare a handshake message with proof of knowledge of the group secret
			let handshake = HandshakeStart {
				network_id,
				group_id: *group_key.id(),
				auth: group_key.generate_proof(&link, local_id),
			};

			// send the handshake to the remote peer
			if let Err(e) = link.send(&handshake).await {
				match e.close_reason() {
					// the remote peer closed the link during handshake because the local
					// node is not known in its discovery catalog. Trigger full discovery
					// catalog sync.
					Some(reason) if reason == UnknownPeer => {
						tracing::trace!(
							peer_id = %Short(peer.id()),
							"remote peer does not recognize us, re-sync catalog",
						);

						let cmd_tx = commands_tx.clone();
						let retry_fut = async move {
							// attempt to re-sync discovery catalog with the remote peer
							if discovery.sync_with(peer.address().clone()).await.is_ok() {
								// after successful sync, attempt to create the bond again
								let _ = cmd_tx.send(Command::CreateBond(peer));
							}
						};
						let _ = commands_tx.send(Command::EnqueueWork(Box::pin(retry_fut)));

						// abort this bonding attempt
						Self::abort_bonding(link, network_id, UnknownPeer).await;
					}

					// the remote peer rejected our authentication proof
					Some(reason) if reason == InvalidAuth => {
						tracing::warn!(
							network = %network_id,
							peer = %Short(peer.id()),
							group = %Short(group_key.id()),
							"remote peer rejected our bond authentication",
						);
						Self::abort_bonding(link, network_id, InvalidAuth).await;
					}

					// bonding failed for other reasons
					_ => {
						tracing::warn!(
							network = %network_id,
							peer = %Short(peer.id()),
							group = %Short(group_key.id()),
							error = ?e,
							"bond handshake rejected by remote peer",
						);
						Self::abort_bonding(link, network_id, UnexpectedClose).await;
					}
				}
				return;
			}

			// wait for the handshake response from the remote peer
			let recv_fut = link.recv::<HandshakeEnd>();
			let Ok(confirm) = recv_fut.await.inspect_err(|e| {
				tracing::debug!(
					network = %network_id,
					peer = %Short(peer.id()),
					group = %Short(group_key.id()),
					error = %e,
					"failed to receive bond handshake response",
				);
			}) else {
				return;
			};

			// validate the authentication proof sent by the remote peer
			if !group_key.validate_proof(&link, confirm.auth) {
				tracing::warn!(
					network = %network_id,
					peer = %Short(peer.id()),
					group = %Short(group_key.id()),
					"remote peer sent invalid bond authentication",
				);
				Self::abort_bonding(link, network_id, InvalidAuth).await;
				return;
			}

			// bond established, save link.
			let peer_id = *peer.id();
			match active.entry(peer_id) {
				Entry::Vacant(entry) => {
					// create a new bond instance to manage this connection and a future
					// that will notify when the bond is terminated
					let (bond, events) =
						Bond::new(link, peer, Arc::clone(&config), cancel.child_token());
					entry.insert(bond);

					// subscribe to bond events
					let cmd = Command::SubscribeToBond(events, peer_id);
					let _ = commands_tx.send(cmd);

					tracing::debug!(
						network = %network_id,
						peer = %Short(peer_id),
						group = %Short(group_key.id()),
						"established peer bond",
					);
				}
				Entry::Occupied(_) => {
					// during this bonding process, another bond was established with
					// this peer, most likely because both peers attempted to connect
					// simultaneously.
					Self::abort_bonding(link, network_id, AlreadyBonded).await;
				}
			}
		}
	}

	/// Given an incoming link and decoded handshake, begins the process of
	/// accepting the bond connection for this group. See [`Handle::accept`].
	fn accept_bond(
		&self,
		mut link: Link<Groups>,
		handshake: HandshakeStart,
		result_tx: oneshot::Sender<Result<(), AcceptError>>,
	) -> impl Future<Output = ()> + Send + Sync + 'static {
		let config = Arc::clone(&self.config);
		let active: Arc<DashMap<PeerId, Bond>> = Arc::clone(&self.active);
		let catalog = self.discovery.catalog();
		let handle = Arc::clone(&self.handle);
		let local = self.local.clone();
		let peer_id = link.remote_id();
		let cancel = self.cancel.clone();
		let network_id = *self.handle.network_id();
		let commands_tx = self.commands_tx.clone();

		async move {
			macro_rules! abort {
				($reason:expr) => {
					WorkerLoop::abort_bonding(link, network_id, $reason).await
				};
			}

			// Ensures that the initiating peer is not already connected to this node
			// in this group.
			// There is no predefined logic that decides which peer should initiate
			// the connection and which should accept it. The first peer that
			// discovers the other and initiates the connection will be accepted,
			// while the other peer's attempt will be rejected.
			if active.contains_key(&peer_id) {
				tracing::trace!(
					network = %local.network_id(),
					peer = %Short(peer_id),
					group = %Short(handle.key.id()),
					"rejecting incoming bond: already bonded",
				);
				let _ = result_tx.send(Err(abort!(AlreadyBonded)));
				return;
			}

			// Ensure that the initiating peer is known in the discovery catalog.
			// If the peer is not known, the connection is aborted. And the initiating
			// peer will need to re-sync its catalog and retry the connection.
			let Some(peer) = catalog.get(&peer_id).cloned() else {
				tracing::trace!(
					peer_id = %Short(&peer_id),
					"rejecting unidentified group member",
				);

				let _ = result_tx.send(Err(abort!(UnknownPeer)));
				return;
			};

			// Validates the authentication proof of knowledge of the group secret
			// that is sent by the initiating peer during the handshake process.
			if !handle.key().validate_proof(&link, handshake.auth) {
				tracing::warn!(
					network = %local.network_id(),
					peer = %Short(link.remote_id()),
					group = %handle.key.id(),
					"invalid group authentication attempt",
				);
				let _ = result_tx.send(Err(abort!(InvalidAuth)));
				return;
			}

			// After successfully validating the handshake from the initiating peer,
			// the accepting peer also sends its proof of knowledge of the group
			// secret back to the initiator and signals that a link has been
			// successfully established.
			let confirm = HandshakeEnd {
				auth: handle.key().generate_proof(&link, local.id()),
			};

			if let Err(e) = link.send(&confirm).await {
				tracing::debug!(
					network = %local.network_id(),
					peer = %Short(link.remote_id()),
					group = %Short(handle.key.id()),
					error = %e,
					"failed to send handshake response",
				);

				let _ = result_tx.send(Err(abort!(UnexpectedClose)));
				return;
			}

			// bond established, save link.
			match active.entry(peer_id) {
				Entry::Vacant(entry) => {
					// create a new bond instance to manage this connection and a future
					// that will notify when the bond is terminated
					let (bond, events) =
						Bond::new(link, peer, Arc::clone(&config), cancel.child_token());

					entry.insert(bond);

					tracing::debug!(
						network = %local.network_id(),
						peer = %Short(peer_id),
						group = %Short(handle.key.id()),
						"accepted peer bond",
					);

					// subscribe to bond events
					let cmd = Command::SubscribeToBond(events, peer_id);
					let _ = commands_tx.send(cmd);

					// signal successful acceptance of the bond externally
					let _ = result_tx.send(Ok(()));
				}
				Entry::Occupied(_) => {
					// during this handshake process, another bond was established with
					// this peer, most likely because both peers attempted to connect
					// simultaneously.
					let _ = result_tx.send(Err(abort!(AlreadyBonded)));
				}
			}
		}
	}

	/// Terminates a bonding connection during the handshake process due to an
	/// error. This closes the link with the remote peer using the provided
	/// application-level close reason and returns an `AcceptError` that can be
	/// returned directly from the `accept_bond` method.
	async fn abort_bonding(
		link: Link<Groups>,
		network_id: NetworkId,
		reason: impl CloseReason,
	) -> AcceptError {
		let remote_id = link.remote_id();
		let app_reason: ApplicationClose = reason.clone().into();
		if let Err(e) = link.close(app_reason.clone()).await {
			tracing::debug!(
				network = %network_id,
				peer = %Short(remote_id),
				error = %e,
				"failed to close link during handshake abort",
			);
			return AcceptError::from_err(e);
		}

		AcceptError::from_err(reason)
	}
}

/// Commands sent to the worker loop.
enum Command {
	/// Accepts an incoming connection for this group.
	/// Connections that are routed here have already passed preliminary
	/// validation such as network id and group id checks.
	Accept(
		Link<Groups>,
		HandshakeStart,
		oneshot::Sender<Result<(), AcceptError>>,
	),

	/// When a bond is created, its event receiver is sent to the worker loop
	SubscribeToBond(mpsc::UnboundedReceiver<BondEvent>, PeerId),

	/// Enqueues arbitrary asynchronous work to be processed by the worker
	/// loop.
	EnqueueWork(Pin<Box<dyn Future<Output = ()> + Send + Sync + 'static>>),

	/// Initiates creating a new bond connection to a remote peer in the group.
	CreateBond(PeerEntry),
}

type AsyncWorkQueue =
	FuturesUnordered<Pin<Box<dyn Future<Output = ()> + Send + Sync + 'static>>>;

/// Aggregated stream of bond events from all active bonds in the group.
type BondEventsStream = SelectAll<
	Pin<Box<dyn Stream<Item = (BondEvent, PeerId)> + Send + Sync + 'static>>,
>;
