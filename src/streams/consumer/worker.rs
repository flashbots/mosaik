use {
	super::{
		super::{Criteria, Streams},
		Consumer,
		Datum,
		Status,
	},
	crate::{
		discovery::{Catalog, Discovery, PeerEntry},
		network::{LocalNode, PeerId, link::CloseReason},
		streams,
	},
	core::pin::Pin,
	futures::{Stream, StreamExt, future::JoinAll, stream::SelectAll},
	std::collections::HashMap,
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
	status_rx: StatusUpdatesStream,
}

impl<D: Datum> ConsumerWorker<D> {
	/// Spawns a new receive worker task and returns the consumer handle that can
	/// receive data and query status.
	///
	/// The worker handle will terminate when the returned consumer is dropped.
	pub fn spawn(
		local: &LocalNode,
		discovery: &Discovery,
		_config: &streams::Config,
		criteria: Criteria,
	) -> Consumer<D> {
		let local = local.clone();
		let cancel = local.termination().child_token();
		let (data_tx, data_rx) = mpsc::unbounded_channel();

		// Get a watch handle for the discovery catalog
		// and mark it as changed to trigger an initial producers lookup in the
		// current state of the catalog.
		let mut catalog = discovery.catalog_watch();
		catalog.mark_changed();

		let worker = ConsumerWorker {
			local,
			data_tx,
			criteria,
			catalog,
			cancel: cancel.clone(),
			active: HashMap::new(),
			status_rx: StatusUpdatesStream::new(),
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
	fn on_catalog_update(&mut self, latest: Catalog) {
		let stream_id = D::stream_id();

		// identify all producers that are producing the desired stream id
		let producers = latest
			.peers()
			.filter(|peer| peer.streams().contains(&stream_id));

		for producer in producers {
			// for each discovered producer, create a receive worker if not
			// already active
			if !self.active.contains_key(producer.id()) {
				tracing::info!(
					peer = %producer.id(),
					stream_id = %stream_id,
					"Discovered new producer for stream"
				);

				// spawn a new receiver worker for this producer
				let receiver = Receiver::spawn(producer.clone(), self);

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
		tracing::info!(
			peer = %peer_id,
			state = ?state,
			stream_id = %D::stream_id(),
			criteria = ?self.criteria,
			"Receiver state updated"
		);

		if state == State::Terminated {
			// remove terminated receiver from active list
			self.active.remove(&peer_id);
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

/// Worker task that manages receiving data from one remote producer peer.
struct Receiver<D: Datum> {
	local: LocalNode,
	peer: PeerEntry,
	criteria: Criteria,
	data_tx: mpsc::UnboundedSender<D>,
	state_tx: watch::Sender<State>,
	cancel: CancellationToken,
}

/// Controls and observes the state of one stream receiver worker associated
/// with a remote producer.
///
/// There should be only one instance of this worker handle per remote
/// producer peer for a given consumer worker.
struct ReceiverHandle {
	cancel: CancellationToken,
	state: watch::Receiver<State>,
}

impl ReceiverHandle {
	/// Terminates the receiver worker and waits for it to shut down.
	pub async fn terminate(mut self) {
		let mut state = *self.state.borrow_and_update();

		loop {
			match state {
				State::Terminated => break,
				State::Terminating => {
					if self.state.changed().await.is_err() {
						break;
					}
					state = *self.state.borrow_and_update();
				}
				_ => {
					self.cancel.cancel();
					if self.state.changed().await.is_err() {
						break;
					}
					state = *self.state.borrow_and_update();
				}
			}
		}
	}

	/// Returns a watch handle for monitoring the receiver state.
	pub const fn state(&self) -> &watch::Receiver<State> {
		&self.state
	}
}

impl<D: Datum> Receiver<D> {
	pub fn spawn(
		peer: PeerEntry,
		consumer: &ConsumerWorker<D>,
	) -> ReceiverHandle {
		let local = consumer.local.clone();
		let cancel = consumer.cancel.child_token();
		let data_tx = consumer.data_tx.clone();
		let (state_tx, state) = watch::channel(State::Connecting);

		let worker = Receiver {
			local,
			peer,
			criteria: consumer.criteria.clone(),
			data_tx,
			state_tx,
			cancel: cancel.clone(),
		};

		tokio::spawn(worker.run());

		ReceiverHandle { cancel, state }
	}
}

impl<D: Datum> Receiver<D> {
	pub async fn run(self) {
		let mut link = self
			.local
			.connect(self.peer.address().clone(), Streams::ALPN)
			.await
			.unwrap();

		loop {
			tokio::select! {
				// Triggered when the consumer is dropped or the network is shutting down
				// or an unrecoverable error occurs with this receiver
				() = self.cancel.cancelled() => {
					self.state_tx.send(State::Terminating).ok();
					link.close_with_reason(CloseReason::Unspecified).await.ok();
					self.state_tx.send(State::Terminated).ok();
					return;
				}

				// Triggered when new data is received from the remote producer
				item = link.recv_as::<D>() => {

				}
			}
		}
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
	Connecting,
	Connected,
	Terminating,
	Terminated,
}

type StatusUpdatesStream =
	SelectAll<Pin<Box<dyn Stream<Item = (State, PeerId)> + Send>>>;
