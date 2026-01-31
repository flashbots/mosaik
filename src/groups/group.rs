use {
	crate::{
		GroupKey,
		Groups,
		NetworkId,
		PeerId,
		discovery::{Catalog, Discovery, PeerEntryVersion, SignedPeerEntry},
		groups::{
			Config,
			GroupId,
			accept::HandshakeStart,
			bond::{Bond, BondEvent, BondWorker, WireMessage},
			error::AlreadyBonded,
		},
		network::{LocalNode, link::Link},
		primitives::{AsyncWorkQueue, Short},
	},
	core::pin::Pin,
	futures::{Stream, StreamExt, stream::SelectAll},
	im::ordmap::Entry,
	iroh::protocol::AcceptError,
	std::sync::Arc,
	tokio::sync::{
		mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
		oneshot,
		watch,
	},
	tokio_stream::wrappers::UnboundedReceiverStream,
	tokio_util::sync::CancellationToken,
};

/// Represents one group instance that the local node is a member of.
///
/// Notes:
///
/// - This type is cheap to clone as it uses an Arc internally and all clones
///   refer to the same underlying group instance.
///
/// - A node can be a member of multiple groups simultaneously, but it can only
///   have one active group instance per unique group id. Attempting to join the
///   same group id multiple times will return the existing instance.
///
/// - Each group instance has a background worker loop that manages changes to
///   the group's current state, including active bonds to other peers in the
///   group.
///
/// - Any changes to the local node's `PeerEntry` is immediately broadcasted to
///   all active bonds in the group outside of the discovery subsystem.
#[derive(Clone)]
pub struct Group(Arc<GroupState>);

impl Group {
	/// Returns the unique identifier for this group that is derived from the
	/// group key.
	pub fn id(&self) -> &GroupId {
		self.key().id()
	}

	/// Returns the group key associated with this group.
	///
	/// The group key defines the authentication parameters for the group
	/// and is used to derive the group id.
	pub fn key(&self) -> &GroupKey {
		&self.0.key
	}

	/// Returns the network id this group belongs to.
	pub fn network_id(&self) -> &NetworkId {
		self.0.local.network_id()
	}

	/// Returns a watch receiver that can be used to observe changes to the
	/// currently active bonds in the group.
	pub fn bonds(&self) -> Bonds {
		self.0.active.clone()
	}
}

/// Internal API
impl Group {
	/// Called by the public Groups API when the local node is joining a new
	/// group. If this is the first call to join this group id, a new group
	/// instance is created and a background worker loop is spawned to manage it.
	/// Otherwise, a handle to the existing group instance is returned.
	pub(super) fn new(groups: &Groups, key: GroupKey) -> Self {
		Self(WorkerLoop::spawn(groups, key))
	}

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
	pub(super) async fn accept(
		&self,
		link: Link<Groups>,
		peer: SignedPeerEntry,
		handshake: HandshakeStart,
	) -> Result<(), AcceptError> {
		self.0.accept(link, peer, handshake).await
	}
}

/// Manages the internal state of a group instance.
///
/// Parts of this type are exposed publicly via the `Group` handle. All copies
/// of the `Group` for the same group id will reference the same instance of
/// this type.
pub(super) struct GroupState {
	/// The groups subsystem configuration.
	pub config: Arc<Config>,

	/// The group key associated with this group.
	pub key: GroupKey,

	/// Reference to the local node networking stack instance.
	///
	/// This is needed to initiate outgoing connections to other peers in the
	/// group when they are discovered and bonds need to be created.
	pub local: LocalNode,

	/// Channel for sending commands to the worker loop.
	pub commands_tx: UnboundedSender<Command>,

	/// List of all active bonds in this group. Each bond represents a direct
	/// connection to another peer in the group. Bonds that are in the process of
	/// being established or are disabled are also tracked here.
	pub active: Bonds,

	/// A reference to the discovery service for peer discovery and peers
	/// catalog.
	pub discovery: Discovery,

	/// Cancellation token that terminates the worker loop for this group and all
	/// active bonds associated with it.
	pub cancel: CancellationToken,
}

/// Internal API
impl GroupState {
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
	pub(super) async fn accept(
		&self,
		link: Link<Groups>,
		peer: SignedPeerEntry,
		handshake: HandshakeStart,
	) -> Result<(), AcceptError> {
		let (result_tx, result_rx) = oneshot::channel();
		let command = Command::Accept(link, peer, handshake, result_tx);

		// handoff the accept process to the background worker loop
		self
			.commands_tx
			.send(command)
			.map_err(AcceptError::from_err)?;

		// wait for the worker loop to process the accept request
		result_rx.await.map_err(AcceptError::from_err)?
	}

	/// Sends an external command to the worker loop managing this group.
	pub(super) fn send_command(&self, command: Command) {
		let _ = self.commands_tx.send(command);
	}
}

/// Commands sent to the worker loop.
#[expect(clippy::large_enum_variant)]
pub(in crate::groups) enum Command {
	/// Accepts an incoming connection for this group.
	/// Connections that are routed here have already passed preliminary
	/// validation such as network id and group id checks.
	Accept(
		Link<Groups>,
		SignedPeerEntry,
		HandshakeStart,
		oneshot::Sender<Result<(), AcceptError>>,
	),

	/// When a bond is created, its event receiver is sent to the worker loop
	SubscribeToBond(UnboundedReceiver<BondEvent>, PeerId),

	/// Enqueues arbitrary asynchronous work to be processed by the worker
	/// loop.
	EnqueueWork(Pin<Box<dyn Future<Output = ()> + Send + Sync + 'static>>),
}

/// Background worker loop that manages the state of a group and reacts to
/// changes in the group's state, such as new peers being discovered or bonds
/// changing state.
struct WorkerLoop {
	/// The internal shared state of the group instance.
	state: Arc<GroupState>,

	/// Aggregated stream of all events emitted by active bonds in the group.
	events: BondEventsStream,

	/// Channel for receiving commands to be processed by the worker loop.
	commands_rx: UnboundedReceiver<Command>,

	/// Pending async work to be processed by the worker loop.
	work_queue: AsyncWorkQueue,

	/// The latest version of the local peer entry known to the group.
	///
	/// This is used to observe changes to the local peer entry and broadcast
	/// updates to all active bonds in the group over established bonds.
	latest_local: PeerEntryVersion,
}

impl WorkerLoop {
	pub fn spawn(groups: &Groups, key: GroupKey) -> Arc<GroupState> {
		let active = Bonds::default();
		let cancel = groups.local.termination().child_token();
		let work_queue = AsyncWorkQueue::new();

		let (commands_tx, commands_rx) = unbounded_channel();

		let state = Arc::new(GroupState {
			key,
			cancel,
			active,
			commands_tx,
			local: groups.local.clone(),
			config: Arc::clone(&groups.config),
			discovery: groups.discovery.clone(),
		});

		let worker = Self {
			commands_rx,
			work_queue,
			state: Arc::clone(&state),
			events: SelectAll::new(),
			latest_local: groups.discovery.me().update_version(),
		};

		tokio::spawn(worker.run());

		state
	}
}

impl WorkerLoop {
	async fn run(mut self) {
		// trigger initial catalog scan for peers in the group
		let mut catalog = self.state.discovery.catalog_watch();
		catalog.mark_changed();

		loop {
			tokio::select! {
				() = self.state.cancel.cancelled() => {
					self.on_terminated();
					break;
				}

				// polls pending async work tasks and drives their execution
				_ = self.work_queue.next() => { }

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
	}

	/// When the group instance is terminated, this happens when the network is
	/// shutting down or when the group is being left.
	fn on_terminated(&self) {
		tracing::warn!(">--> Group {} worker loop terminated", self.state.key.id());
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
			.filter(|peer| peer.groups().contains(self.state.key.id()));

		for peer in new_peers_in_group {
			self.create_bond(peer.clone());
		}

		// Check if our local peer entry has been updated. If so, broadcast
		// the changes to all active bonds in the group.
		let me = self.state.discovery.me();
		if me.update_version() > self.latest_local {
			// our local peer entry has been updated, broadcast the changes
			// to all active bonds in the group.
			self.latest_local = me.update_version();
			self.broadcast(&WireMessage::PeerEntryUpdate(Box::new(me)));
		}
	}

	/// Handles incoming external commands sent to the worker loop.
	fn on_external_command(&mut self, command: Command) {
		match command {
			// Begins the process of accepting an incoming connection for this
			Command::Accept(link, peer, handshake, result_tx) => {
				self.accept_bond(link, peer, handshake, result_tx);
			}
			// Subscribes to bond events from a newly created bond
			Command::SubscribeToBond(events_rx, peer_id) => {
				self.events.push(Box::pin(
					UnboundedReceiverStream::new(events_rx)
						.map(move |event| (event, peer_id)),
				));
			}
			// Enqueues arbitrary asynchronous work to be
			// processed by the worker loop.
			Command::EnqueueWork(fut) => {
				self.work_queue.enqueue(fut);
			}
		}
	}

	/// Broadcasts a wire message to all active bonds in the group.
	fn broadcast(&self, message: &WireMessage) {
		for bond in self.state.active.iter() {
			bond.send(message.clone());
		}
	}

	/// Handles bond events from active bonds in the group.
	fn on_bond_event(&self, event: BondEvent, peer_id: PeerId) {
		match event {
			BondEvent::Terminated(reason) => {
				// remove the bond from the active list
				self.state.active.update_with(|active| {
					if active.remove(&peer_id).is_some() && reason != AlreadyBonded {
						tracing::debug!(
							group = %Short(self.state.key.id()),
							peer = %Short(peer_id),
							network = %self.state.local.network_id(),
							reason = ?reason,
							"bond terminated",
						);
					}
				});
			}
			BondEvent::Connected => {}
		}
	}

	/// Initiates the process of creating a new bond connection to a remote
	/// peer in the group.
	///
	/// This happens in response to discovering a new peer in the group via
	/// the discovery catalog. This method is called only for peers that are
	/// already known in the discovery catalog.
	fn create_bond(&self, peer: SignedPeerEntry) {
		if self.state.active.contains_peer(peer.id()) {
			// there's already an active bond to this peer
			return;
		}

		// initiate a new bond connection with this peer.
		let peer_id = *peer.id();
		let state = Arc::clone(&self.state);
		let fut = async move {
			match BondWorker::create(Arc::clone(&state), peer).await {
				Ok((handle, events)) => {
					state.active.update_with(|active| {
						match active.entry(peer_id) {
							Entry::Vacant(place) => {
								// keep track of the bond handle to control it
								place.insert(handle);

								// subscribe to bond events
								state
									.commands_tx
									.send(Command::SubscribeToBond(events, peer_id))
									.ok();

								tracing::debug!(
									group = %state.key.id(),
									peer = %Short(peer_id),
									network = %state.local.network_id(),
									initiator = true,
									"new bond created",
								);
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
					tracing::trace!(
						network = %state.local.network_id(),
						peer = %Short(peer_id),
						group = %Short(state.key.id()),
						reason = ?reason,
						"failed to create peer bond",
					);
				}
			}
		};

		self.enqueue_work(fut);
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

		if self.state.active.contains_peer(&peer_id) {
			// there's already an active bond to this peer
			tokio::spawn(link.close(AlreadyBonded));
			let _ = result_tx.send(Err(AcceptError::from_err(AlreadyBonded)));
			return;
		}

		let state = Arc::clone(&self.state);
		let fut = async move {
			match BondWorker::accept(Arc::clone(&state), link, peer, handshake).await
			{
				Ok((handle, events)) => {
					state.active.update_with(|active| {
						match active.entry(peer_id) {
							Entry::Vacant(place) => {
								// keep track of the bond handle to control it
								place.insert(handle);

								// subscribe to bond events
								state
									.commands_tx
									.send(Command::SubscribeToBond(events, peer_id))
									.ok();

								tracing::debug!(
									group = %Short(state.key.id()),
									peer = %Short(peer_id),
									network = %state.local.network_id(),
									initiator = false,
									"new bond created",
								);

								let _ = result_tx.send(Ok(()));
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

	fn enqueue_work<F>(&self, fut: F)
	where
		F: Future<Output = ()> + Send + Sync + 'static,
	{
		self.state.send_command(Command::EnqueueWork(Box::pin(fut)));
	}
}

/// Aggregated stream of bond events from all active bonds in the group.
type BondEventsStream = SelectAll<
	Pin<Box<dyn Stream<Item = (BondEvent, PeerId)> + Send + Sync + 'static>>,
>;

/// A watchable collection of currently active bonds in a group.
///
/// This type allows observing changes to the set of active bonds.
#[derive(Clone)]
pub struct Bonds(watch::Sender<im::OrdMap<PeerId, Bond>>);

/// Public API
impl Bonds {
	/// Returns the number of active bonds in the group.
	pub fn len(&self) -> usize {
		self.0.borrow().len()
	}

	/// Returns `true` if there are no active bonds in the group.
	pub fn is_empty(&self) -> bool {
		self.0.borrow().is_empty()
	}

	/// Returns `true` if there is an active bond to the specified peer.
	pub fn contains_peer(&self, peer_id: &PeerId) -> bool {
		self.0.borrow().contains_key(peer_id)
	}

	/// Returns an iterator over all active bonds in the group ordered by their
	/// peer ids at the time of calling this method.
	pub fn iter(&self) -> impl Iterator<Item = Bond> {
		let bonds = self.0.borrow().clone();
		bonds.into_iter().map(|(_, bond)| bond)
	}

	/// Returns a future that resolves when there is a change to the active
	/// bonds in the group.
	pub async fn changed(&self) {
		let _ = self.0.subscribe().changed().await;
	}
}

/// Internal API
impl Default for Bonds {
	fn default() -> Self {
		Self(watch::Sender::new(im::OrdMap::new()))
	}
}

/// Internal API
impl Bonds {
	fn update_with(&self, f: impl FnOnce(&mut im::OrdMap<PeerId, Bond>)) {
		self.0.send_if_modified(|active| {
			let before = active.len();
			f(active);
			active.len() != before
		});
	}
}
