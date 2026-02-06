use {
	crate::{
		Datum,
		PeerId,
		discovery::{Catalog, Discovery},
		network::LocalNode,
		primitives::{Digest, Short},
		streams::{
			Consumer,
			Streams,
			consumer::{builder::ConsumerConfig, receiver::Receiver},
			status::{ActiveChannelsMap, State, Stats, When},
		},
	},
	core::pin::Pin,
	futures::{Stream, StreamExt, stream::SelectAll},
	std::sync::Arc,
	tokio::sync::{mpsc, watch},
	tokio_stream::wrappers::WatchStream,
	tokio_util::sync::CancellationToken,
};

/// Worker task that manages the state of one consumer for a specific datum type
/// `D` on the local node.
///
/// Notes:
/// - Individual worker tasks are spawned for each subscribed producer peer.
pub(super) struct ConsumerWorker<D: Datum> {
	/// The consumer-specific configuration as assembled by
	/// `Network::streams().consumer()`. If some configuration values are not
	/// set, they default to the values from `Streams` config.
	config: Arc<ConsumerConfig>,

	/// A handle to the local node that is used to establish new connections.
	local: LocalNode,

	/// The discovery system handle used to monitor known peers that are
	/// producing the desired stream and also to trigger catalog syncs when
	/// the consumer is connecting to a producer that does not recognize it.
	discovery: Discovery,

	/// Channel for sending received data to the consumer handle.
	data_tx: mpsc::UnboundedSender<(D, usize)>,

	/// Triggered when the consumer handle is dropped.
	cancel: CancellationToken,

	/// Active receive workers for connected producer peers.
	/// This value can be observed to get the current snapshot of connected
	/// producers.
	active: watch::Sender<ActiveChannelsMap>,

	/// Aggregated status streams from all active receiver workers.
	status_rx: StateUpdatesStream,

	/// A one-time set handle that is completed when the consumer worker loop is
	/// ready.
	online: watch::Sender<bool>,
}

impl<D: Datum> ConsumerWorker<D> {
	/// Spawns a new receive worker task and returns the consumer handle that can
	/// receive data and query status.
	///
	/// The worker handle will terminate when the returned consumer is dropped.
	pub fn spawn(config: ConsumerConfig, streams: &Streams) -> Consumer<D> {
		let config = Arc::new(config);
		let local = streams.local.clone();
		let cancel = local.termination().child_token();
		let active = watch::Sender::new(ActiveChannelsMap::new());
		let online = watch::Sender::new(true);
		let (data_tx, data_rx) = mpsc::unbounded_channel();

		let worker = Self {
			local,
			data_tx,
			config: Arc::clone(&config),
			discovery: streams.discovery.clone(),
			cancel: cancel.clone(),
			active: active.clone(),
			status_rx: StateUpdatesStream::new(),
			online: online.clone(),
		};

		tokio::spawn(worker.run());

		Consumer {
			config: Arc::clone(&config),
			chan: data_rx,
			stats: Stats::default_connected(),
			status: When::new(active.subscribe(), online.subscribe()),
			_abort: cancel.drop_guard(),
		}
	}
}

impl<D: Datum> ConsumerWorker<D> {
	async fn run(mut self) {
		// Get a watch handle for the discovery catalog
		// and mark it as changed to trigger an initial producers lookup in the
		// current state of the catalog.
		let mut catalog = self.discovery.catalog_watch();
		catalog.mark_changed();

		// mark the consumer as ready after initial setup is done
		let _ = self.online.send(true);

		loop {
			tokio::select! {
				// Triggered when the consumer is dropped or the network is shutting down
				() = self.cancel.cancelled() => {
					self.on_terminated();
					break;
				}

				// Triggered when new peers are discovered or existing peers are updated
				_ = catalog.changed() => {
					// mark the latest catalog snapshot as seen and trigger peers scan
					let snapshot = catalog.borrow_and_update().clone();
					self.on_catalog_update(snapshot);
				}

				// Triggered when one of the active receiver workers updates its state
				Some((state, peer_id)) = self.status_rx.next() => {
					self.on_receiver_state_update(peer_id, state);
				}
			}
		}
	}

	/// Handles updates to the discovery catalog.
	#[expect(
		clippy::needless_pass_by_value,
		reason = "Catalog is cheaply cloneable and we don't want to hold a lock \
		          on the watcher while processing"
	)]
	fn on_catalog_update(&mut self, latest: Catalog) {
		// identify all producers that are producing the desired stream id
		// and satisfy the user-provided additional eligibility criteria.
		let producers = latest
			.peers()
			.filter(|peer| peer.streams().contains(&self.config.stream_id));

		for producer in producers {
			// for each discovered producer, create a receive worker if it is not
			// already in the active list of connected producers.
			let sub_id = Digest::from_bytes(*producer.id().as_bytes());
			if !self.active.borrow().contains_key(&sub_id) {
				tracing::trace!(
					stream_id = %Short(self.config.stream_id),
					producer = %Short(producer),
					"discovered new stream producer"
				);

				if !(self.config.subscribe_if)(producer) {
					tracing::debug!(
						stream_id = %Short(self.config.stream_id),
						producer_id = %Short(producer),
						"skipping producer that does not satisfy eligibility criteria"
					);
					continue;
				}

				// spawn a new receiver worker for this producer and track its status
				let channel_info = Receiver::spawn(
					producer.clone(),
					&self.local,
					&self.discovery,
					&self.cancel,
					&self.data_tx,
					Arc::clone(&self.config),
				);

				// subscribe to the receiver's status updates
				let peer_id = *producer.id();
				self.status_rx.push(
					WatchStream::new(channel_info.state.clone())
						.map(move |state| (state, peer_id))
						.boxed(),
				);

				// track the active receiver handle, when workers terminate they will
				// transition into the terminated state and be removed from the active
				// list
				self.active.send_modify(|active| {
					active.insert(sub_id, channel_info);
				});
			}
		}
	}

	/// Handles state updates from remote receiver workers.
	fn on_receiver_state_update(&self, peer_id: PeerId, state: State) {
		if state == State::Terminated {
			// The receiver has unrecoverably terminated, remove it from the active
			// list
			let sub_id = Digest::from_bytes(*peer_id.as_bytes());

			self
				.active
				.send_if_modified(|active| active.remove(&sub_id).is_some());

			tracing::info!(
				producer_id = %Short(&peer_id),
				stream_id = %Short(self.config.stream_id),
				criteria = ?self.config.criteria,
				"connection with producer terminated"
			);
		}
	}

	/// Gracefully closes all connections with remote producers.
	fn on_terminated(&self) {
		// terminate all active receiver workers
		let producers_count = self.active.borrow().len();
		self.active.send_replace(ActiveChannelsMap::default());

		tracing::debug!(
			stream_id = %Short(self.config.stream_id),
			producers_count = producers_count,
			criteria = ?self.config.criteria,
			"consumer terminated"
		);
	}
}

type StateUpdatesStream =
	SelectAll<Pin<Box<dyn Stream<Item = (State, PeerId)> + Send>>>;
