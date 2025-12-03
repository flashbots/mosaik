use {
	super::{
		super::{Config, Criteria, Streams},
		Consumer,
		Datum,
		Status,
		receiver::{Receiver, ReceiverHandle, State},
	},
	crate::{
		discovery::Catalog,
		network::{LocalNode, PeerId},
	},
	core::pin::Pin,
	futures::{Stream, StreamExt, future::JoinAll, stream::SelectAll},
	std::{collections::HashMap, sync::Arc},
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
	/// Configuration for the streams subsystem.
	config: Arc<Config>,

	/// A handle to the local node that is used to establish new connections.
	local: LocalNode,

	/// The stream subscription criteria for this consumer.
	criteria: Criteria,

	/// Watch handle for the discovery catalog.
	///
	/// This is used to monitor known peers that are producing the desired
	/// stream.
	catalog: watch::Receiver<Catalog>,

	/// Channel for sending received data to the consumer handle.
	data_tx: mpsc::UnboundedSender<D>,

	/// Triggered when the consumer handle is dropped.
	cancel: CancellationToken,

	/// Active receive workers for connected producer peers.
	active: HashMap<PeerId, ReceiverHandle>,

	/// Aggregated status streams from all active receiver workers.
	status_rx: StateUpdatesStream,
}

impl<D: Datum> ConsumerWorker<D> {
	/// Spawns a new receive worker task and returns the consumer handle that can
	/// receive data and query status.
	///
	/// The worker handle will terminate when the returned consumer is dropped.
	pub fn spawn(streams: &Streams, criteria: Criteria) -> Consumer<D> {
		let local = streams.local.clone();
		let config = Arc::clone(&streams.config);
		let cancel = local.termination().child_token();
		let (data_tx, data_rx) = mpsc::unbounded_channel();

		// Get a watch handle for the discovery catalog
		// and mark it as changed to trigger an initial producers lookup in the
		// current state of the catalog.
		let mut catalog = streams.discovery.catalog_watch();
		catalog.mark_changed();

		let worker = ConsumerWorker {
			config,
			local,
			data_tx,
			criteria,
			catalog,
			cancel: cancel.clone(),
			active: HashMap::new(),
			status_rx: StateUpdatesStream::new(),
		};

		tokio::spawn(worker.run());

		Consumer {
			status: Status,
			chan: data_rx,
			_abort: cancel.drop_guard(),
		}
	}
}

impl<D: Datum> ConsumerWorker<D> {
	async fn run(mut self) {
		loop {
			tokio::select! {
				// Triggered when the consumer is dropped or the network is shutting down
				() = self.cancel.cancelled() => {
					self.on_terminated().await;
					break;
				}

				// Triggered when new peers are discovered or existing peers are updated
				_ = self.catalog.changed() => {
					// mark the latest catalog snapshot as seen and trigger peers scan
					let snapshot = self.catalog.borrow_and_update().clone();
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
		let stream_id = D::stream_id();

		// identify all producers that are producing the desired stream id
		let producers = latest
			.peers()
			.filter(|peer| peer.streams().contains(&stream_id));

		for producer in producers {
			// for each discovered producer, create a receive worker if it is not
			// already in the active list of connected producers.
			if !self.active.contains_key(producer.id()) {
				tracing::debug!(
					stream_id = %stream_id,
					producer_id = %producer.id(),
					"discovered new producer"
				);

				// spawn a new receiver worker for this producer
				let receiver = Receiver::spawn(
					producer.clone(),
					&self.local,
					&self.cancel,
					&self.data_tx,
					&self.config,
					&self.criteria,
				);

				// subscribe to the receiver's status updates
				let peer_id = *producer.id();
				let status_stream = WatchStream::new(receiver.state().clone());
				self
					.status_rx
					.push(status_stream.map(move |state| (state, peer_id)).boxed());

				// track the active receiver handle, when workers terminate they will
				// transition into the terminated state and be removed from the active
				// list
				self.active.insert(*producer.id(), receiver);
			}
		}
	}

	/// Handles state updates from remote receiver workers.
	fn on_receiver_state_update(&mut self, peer_id: PeerId, state: State) {
		if state == State::Terminated {
			self.active.remove(&peer_id);
			tracing::info!(
				producer_id = %peer_id,
				stream_id = %D::stream_id(),
				criteria = ?self.criteria,
				"connection with producer terminated"
			);
		}
	}

	/// Gracefully closes all connections with remote producers.
	async fn on_terminated(&mut self) {
		self
			.active
			.drain()
			.map(|(_peer_id, handle)| handle.terminate())
			.collect::<JoinAll<_>>()
			.await;
	}
}

type StateUpdatesStream =
	SelectAll<Pin<Box<dyn Stream<Item = (State, PeerId)> + Send>>>;
