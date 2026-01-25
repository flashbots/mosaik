use {
	crate::{
		GroupKey,
		Groups,
		NetworkId,
		PeerId,
		discovery::Catalog,
		groups::{AlreadyConnected, Config, Error, GroupId, group::conn::State},
		network::{LocalNode, link::Link},
		primitives::Short,
	},
	conn::Connection,
	core::pin::Pin,
	dashmap::{DashMap, Entry},
	futures::{Stream, StreamExt, stream::SelectAll},
	std::sync::Arc,
	tokio::sync::watch,
	tokio_stream::wrappers::WatchStream,
	tokio_util::sync::CancellationToken,
};

mod conn;
mod worker;

/// Represents one group with a unique group id.
///
/// Notes:
///
/// - This type is cheap to clone as it uses an Arc internally and all clones
///   refer to the same underlying group instance.
#[derive(Clone)]
pub struct Group(Arc<Handle>);

impl Group {
	pub fn members(&self) -> impl Iterator<Item = PeerId> {
		std::iter::empty()
	}

	pub fn has_member(&self, _peer_id: &PeerId) -> bool {
		false
	}

	pub fn id(&self) -> GroupId {
		self.key().id()
	}

	pub fn key(&self) -> &GroupKey {
		&self.0.key
	}
}

/// Internal API
impl Group {
	pub(super) fn new(groups: &Groups, key: GroupKey) -> Self {
		let handle = WorkerLoop::spawn(
			key,
			&groups.local,
			groups.discovery.catalog_watch(),
			Arc::clone(&groups.config),
		);

		Self(handle)
	}

	pub(super) fn accept(
		&self,
		link: Link<Groups>,
	) -> Result<(), (Link<Groups>, AlreadyConnected)> {
		let peer_id = link.remote_id();
		match self.0.active.entry(peer_id) {
			Entry::Occupied(_) => Err((link, AlreadyConnected)),
			Entry::Vacant(entry) => {
				tracing::debug!(
					network = %self.0.network_id,
					group = %Short(self.0.key.id()),
					peer = %Short(peer_id),
					"group link established",
				);

				Ok(())
			}
		}
	}
}

pub(super) struct Handle {
	key: GroupKey,
	network_id: NetworkId,
	active: Arc<DashMap<PeerId, Connection>>,
}

pub(super) struct WorkerLoop {
	config: Arc<Config>,
	handle: Arc<Handle>,
	local: LocalNode,
	cancel: CancellationToken,
	catalog: watch::Receiver<Catalog>,
	active: Arc<DashMap<PeerId, Connection>>,
	events: ConnectionEventsStream,
}

impl WorkerLoop {
	pub(super) fn spawn(
		key: GroupKey,
		local: &LocalNode,
		catalog: watch::Receiver<Catalog>,
		config: Arc<Config>,
	) -> Arc<Handle> {
		let active = Arc::new(DashMap::new());
		let cancel = local.termination().child_token();

		let handle = Arc::new(Handle {
			key,
			network_id: *local.network_id(),
			active: Arc::clone(&active),
		});

		let worker = Self {
			config,
			catalog,
			cancel,
			active,
			local: local.clone(),
			handle: Arc::clone(&handle),
			events: SelectAll::new(),
		};

		tokio::spawn(worker.run());

		handle
	}
}

impl WorkerLoop {
	async fn run(mut self) -> Result<(), Error> {
		// trigger initial catalog scan for peers in the group
		self.catalog.mark_changed();

		loop {
			tokio::select! {
				() = self.cancel.cancelled() => {
					self.on_terminated();
					break;
				}

				_ = self.catalog.changed() => {
					let catalog = self.catalog.borrow_and_update().clone();
					self.on_catalog_update(catalog);
				}

				Some((event, peer_id)) = self.events.next() => {
					self.on_peer_event(peer_id, event);
				}
			}
		}
		Ok(())
	}

	fn on_terminated(&self) {}

	fn on_catalog_update(&mut self, snapshot: Catalog) {
		// Find new peers that have joined the group but are not yet tracked
		// by this worker loop.
		let new_peers_in_group = snapshot.peers().filter(|peer| {
			peer.groups().contains(&self.handle.key.id())
				&& !self.active.contains_key(peer.id())
		});

		for peer in new_peers_in_group {
			let id = *peer.id();
			if let Entry::Vacant(entry) = self.active.entry(id) {
				// this is a new peer in the group, register it and start
				// connecting keep track of the connection handle
				let connection = entry
					.insert(Connection::connect(
						Arc::clone(&self.config),
						peer.address().clone(),
						self.local.clone(),
						self.handle.key.clone(),
						self.cancel.child_token(),
					))
					.downgrade();

				// and monitor all events emitted by this connection
				self.events.push(Box::pin(
					WatchStream::new(connection.state_watch())
						.map(move |event| (event, id)),
				));
			}
		}
	}

	fn on_peer_event(&self, peer_id: PeerId, event: State) {
		match event {
			State::Connecting => {
				tracing::debug!(
					group_id = %self.handle.key.id(),
					peer_id = %Short(peer_id),
					"connecting to group peer",
				);
			}
			State::Connected => {
				tracing::debug!(
					group_id = %self.handle.key.id(),
					peer_id = %Short(peer_id),
					"connected to group peer",
				);
			}
			State::Disconnected => {
				tracing::debug!(
					group_id = %self.handle.key.id(),
					peer_id = %Short(peer_id),
					"disconnected from group peer",
				);
				self.active.remove(&peer_id);
			}
		}
	}
}

type ConnectionEventsStream =
	SelectAll<Pin<Box<dyn Stream<Item = (State, PeerId)> + Send>>>;
