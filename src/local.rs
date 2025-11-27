use {
	crate::{
		prelude::{Datum, NetworkId, PeerInfo, SignedPeerInfo, StreamId, Tag},
		streams::FanoutSink,
	},
	dashmap::{DashMap, Entry},
	futures::StreamExt,
	iroh::{
		Endpoint,
		EndpointAddr,
		Watcher,
		discovery::static_provider::StaticProvider,
	},
	std::sync::Arc,
	tokio::sync::watch,
	tokio_util::sync::{CancellationToken, DropGuard},
	tracing::{debug, warn},
};

/// This type maintains an up to date state about the local peer.
///
/// Notes:
///
/// - This type is cheap to clone; all clones refer to the same underlying local
///   peer state.
///
/// - This type spawns an async event loop task that is responsible for
///   monitoring changes to the local peer's state (e.g. addresses, produced
///   streams).
pub struct Local(Arc<Inner>);

impl Clone for Local {
	fn clone(&self) -> Self {
		Self(Arc::clone(&self.0))
	}
}

impl Local {
	pub fn new(
		endpoint: Endpoint,
		network_id: NetworkId,
		tags: impl Iterator<Item = Tag>,
		static_provider: StaticProvider,
		cancel: CancellationToken,
	) -> Self {
		let initial = PeerInfo::new(endpoint.addr())
			.add_tags(tags)
			.sign(endpoint.secret_key());

		let sinks = DashMap::new();
		let (info_tx, latest) = watch::channel(initial.clone());

		let event_loop = EventLoop {
			info: initial,
			info_tx: info_tx.clone(),
			endpoint: endpoint.clone(),
			cancel: cancel.clone(),
		};

		tokio::spawn(event_loop.run());

		Self(Arc::new(Inner {
			endpoint,
			static_provider,
			network_id,
			sinks,
			latest,
			info_tx,
			_abort: cancel.drop_guard(),
		}))
	}

	/// Returns the transport layer endpoint of the local peer.
	pub fn endpoint(&self) -> &Endpoint {
		&self.0.endpoint
	}

	pub fn static_provider(&self) -> &StaticProvider {
		&self.0.static_provider
	}

	/// Returns all known addresses and relay nodes of the local peer.
	pub fn addr(&self) -> EndpointAddr {
		self.0.endpoint.addr()
	}

	/// Returns the public key of the local peer.
	///
	/// This is a unique global identifier for this peer that should be enough to
	/// identify, discover and connect to it.
	pub fn id(&self) -> iroh::EndpointId {
		self.0.endpoint.id()
	}

	/// Returns the `NetworkId` of this local peer.
	pub fn network_id(&self) -> &NetworkId {
		&self.0.network_id
	}

	/// Awaits until the local peer is online.
	///
	/// When it is online it means that it is registered with discovery and can
	/// accept incoming connections.
	pub async fn online(&self) {
		self.0.endpoint.online().await;
	}

	/// Returns a watch receiver that yields updates to the local peer's info.
	pub fn changes(&self) -> watch::Receiver<SignedPeerInfo> {
		self.0.latest.clone()
	}

	/// Returns the latest known `PeerInfo` for the local peer.
	pub fn info(&self) -> SignedPeerInfo {
		self.0.latest.borrow().clone()
	}

	/// Creates a new sink for a given stream ID.
	///
	/// If there is an existing sink for the stream ID on the local node already
	/// created, a handle to it is returned. The handle can be used to produce
	/// data for that stream.
	pub fn create_sink<D: Datum>(&self) -> FanoutSink {
		let stream_id = StreamId::of::<D>();
		match self.0.sinks.entry(stream_id) {
			Entry::Occupied(occupied) => occupied.get().clone(),
			Entry::Vacant(vacant) => {
				// Create a new sink for this stream ID
				let sink = FanoutSink::new::<D>();
				vacant.insert(sink.clone());

				// Update the local peer info to include this stream ID
				let latest = self.0.latest.borrow().clone();
				let updated = latest
					.info
					.add_stream(sink.stream_id().clone())
					.sign(self.0.endpoint.secret_key());
				let _ = self.0.info_tx.send(updated);

				// Return the newly created sink
				sink
			}
		}
	}

	/// Returns an existing sink for a given stream ID, if one exists, otherwise
	/// returns None.
	pub fn open_sink(&self, stream_id: &StreamId) -> Option<FanoutSink> {
		self
			.0
			.sinks
			.get(stream_id)
			.map(|entry| entry.value().clone())
	}

	/// Adds a tag to the local peer info and notifies watchers of the change.
	/// If the tag already exists, no change is made.
	pub fn add_tag(&self, tag: Tag) {
		let latest = self.0.latest.borrow().clone();
		if self.0.latest.borrow().tags().contains(&tag) {
			return;
		}
		let updated = latest.info.add_tag(tag).sign(self.0.endpoint.secret_key());
		let _ = self.0.info_tx.send(updated);
	}

	/// Removes a tag from the local peer info and notifies watchers of the
	/// change. If the tag does not exist, no change is made.
	pub fn remove_tag(&self, tag: &Tag) {
		let latest = self.0.latest.borrow().clone();
		if !self.0.latest.borrow().tags().contains(tag) {
			return;
		}
		let updated = latest
			.info
			.remove_tag(tag)
			.sign(self.0.endpoint.secret_key());
		let _ = self.0.info_tx.send(updated);
	}
}

struct Inner {
	endpoint: Endpoint,
	static_provider: StaticProvider,
	network_id: NetworkId,
	sinks: DashMap<StreamId, FanoutSink>,
	latest: watch::Receiver<SignedPeerInfo>,
	info_tx: watch::Sender<SignedPeerInfo>,
	_abort: DropGuard,
}

struct EventLoop {
	info: SignedPeerInfo,
	info_tx: watch::Sender<SignedPeerInfo>,
	endpoint: Endpoint,
	cancel: CancellationToken,
}

impl EventLoop {
	async fn run(mut self) {
		let mut addrs_stream = self.endpoint.watch_addr().stream_updates_only();

		loop {
			tokio::select! {
				() = self.cancel.cancelled() => {
					self.on_terminated();
					break;
				}

				// Handle discovered own addresses
				Some(addr) = addrs_stream.next() => self.on_local_addr_changed(addr),
			}
		}
	}

	#[allow(clippy::unused_self)]
	fn on_terminated(&mut self) {
		debug!("Local peer info event loop is terminating");
	}

	fn on_local_addr_changed(&mut self, addr: EndpointAddr) {
		debug!("Discovered new local address: {addr:?}");
		if self.info.address() != &addr {
			let latest = self.info.info.clone().update_address(addr);
			self.info = latest.sign(self.endpoint.secret_key());

			self.info_tx.send(self.info.clone()).unwrap_or_else(|_| {
				warn!("Network terminated; nobody is watching local PeerInfo updates");
				self.cancel.cancel();
			});
		}
	}
}
