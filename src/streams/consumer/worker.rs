use {
	super::{
		super::{Config, Criteria, Streams},
		Consumer,
		Datum,
		Status,
	},
	crate::{
		discovery::{Catalog, PeerEntry},
		network::{
			LocalNode,
			PeerId,
			link::{CloseReason, Link},
		},
		streams::accept::ConsumerHandshake,
	},
	backoff::future::retry,
	core::{future::pending, pin::Pin},
	futures::{Stream, StreamExt, future::JoinAll, stream::SelectAll},
	std::{collections::HashMap, io, sync::Arc},
	tokio::sync::{mpsc, watch},
	tokio_stream::wrappers::WatchStream,
	tokio_util::sync::{CancellationToken, ReusableBoxFuture},
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
	status_rx: StatusUpdatesStream,
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
			// for each discovered producer, create a receive worker if not
			// already active
			if !self.active.contains_key(producer.id()) {
				tracing::debug!(
					stream_id = %stream_id,
					peer = %producer.id(),
					"discovered new producer"
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
	/// Configuration for the streams subsystem.
	config: Arc<Config>,

	/// Local socket, used to initiate connections to remote peers on the Streams
	/// protocol.
	local: LocalNode,

	/// The remote producer peer entry snapshot as known at the time of worker
	/// creation.
	peer: PeerEntry,

	/// The stream subscription criteria for this consumer.
	criteria: Criteria,

	/// Channel for sending received data to the consumer handle for the public
	/// api to consume.
	data_tx: mpsc::UnboundedSender<D>,

	/// Watch channel for reporting the current state of this receiver worker
	/// connection with the remote producer.
	state_tx: watch::Sender<State>,

	/// Triggered when the receiver worker should shut down.
	cancel: CancellationToken,

	/// Reusable future for receiving the next datum from the remote producer.
	///
	/// The receiver future always carries the current physical link along with
	/// it to enable repairing dropped connections according to the backoff
	/// policy.
	next_recv: ReusableBoxFuture<'static, (Result<D, io::Error>, Link)>,
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
		let config = Arc::clone(&consumer.config);
		let (state_tx, state) = watch::channel(State::Connecting);

		let worker = Receiver {
			config,
			local,
			peer,
			criteria: consumer.criteria.clone(),
			data_tx,
			state_tx,
			cancel: cancel.clone(),
			next_recv: ReusableBoxFuture::new(pending()),
		};

		tokio::spawn(worker.run());

		ReceiverHandle { cancel, state }
	}
}

impl<D: Datum> Receiver<D> {
	pub async fn run(mut self) {
		// initial connection attempt
		self.connect(None).await;

		loop {
			tokio::select! {
				// Triggered when the consumer is dropped or the network is shutting down
				// or an unrecoverable error occurs with this receiver
				() = self.cancel.cancelled() => {
					let mut state_watch = self.state_tx.subscribe();
					self.state_tx.send(State::Terminating).ok();

					// wait until the state is observed as terminated
					while *state_watch.borrow_and_update() != State::Terminated {
						if state_watch.changed().await.is_err() {
							break;
						}
					}
				}

				// Triggered when new data is received from the remote producer
				(result, link) = &mut self.next_recv => {
					self.on_next_recv(result, link).await;
				}
			}
		}
	}

	/// Handles the result of receiving the next datum from the remote producer.
	///
	/// Ensures that the next receive future is properly set up for the next datum
	/// and honors the cancellation signal.
	async fn on_next_recv(&mut self, result: Result<D, io::Error>, link: Link) {
		match result {
			// a datum was successfully received
			Ok(datum) => {
				// forward the received datum to the consumer worker
				// for delivery to public api consumer handle.
				self.data_tx.send(datum).ok();

				// if not cancelled, prepare to receive the next datum
				if !self.cancel.is_cancelled() {
					self.next_recv.set(self.make_next_recv_future(link));
				}
			}
			// an error occurred while receiving the datum,
			// unless cancelled, drop the current link and attempt to
			// reconnect according to the backoff policy.
			Err(e) => {
				// explicitly cancelled through the cancellation token
				if e.kind() == io::ErrorKind::Interrupted {
					let _ = link.close_with_reason(CloseReason::Unspecified).await;
					self.state_tx.send(State::Terminated).ok();
					return;
				}

				// io error occurred, drop the current link and attempt to reconnect
				tracing::warn!(
					error = %e,
					stream_id = %D::stream_id(),
					peer = %self.peer.id(),
					"stream receive error, attempting to reconnect",
				);
				self.connect(Some(link)).await;
			}
		}
	}

	/// Creates a future that receives the next datum from the remote producer
	/// over the specified link.
	///
	/// The future is cancellable using the worker's cancellation token and
	/// carries the link along with it for further receives or reconnections.
	fn make_next_recv_future(
		&self,
		mut link: Link,
	) -> impl Future<Output = (Result<D, io::Error>, Link)> + 'static {
		let cancel = self.cancel.clone();
		async move {
			let cancellable_recv = cancel
				.run_until_cancelled_owned(link.recv_as::<D>())
				.await
				.unwrap_or_else(|| {
					Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"))
				});
			(cancellable_recv, link)
		}
	}

	/// Attempts to connect to the remote producer and perform the stream
	/// subscription handshake. This method will retry connections according
	/// to the backoff policy specified in the configuration.
	async fn connect(&mut self, prev: Option<Link>) {
		self.state_tx.send(State::Connecting).ok();

		if let Some(link) = prev {
			// close the previous link before attempting to reconnect
			let _ = link.close_with_reason(CloseReason::Unspecified).await;
		}

		let criteria = &self.criteria;
		let peer_addr = self.peer.address();
		let backoff = (self.config.backoff)();

		// create a connect-and-handshake future with retries according to
		// the backoff policy
		let mut attempt = 0;
		let result: Result<Link, _> = retry(backoff, || {
			attempt += 1;
			let criteria = criteria.clone();
			let cancel = self.cancel.clone();
			let connect_fut = self.local.connect(peer_addr.clone(), Streams::ALPN);

			let cancelled_error = || {
				io::Error::new(
					io::ErrorKind::Interrupted,
					"connection attempt cancelled",
				)
			};

			async move {
				if cancel.is_cancelled() {
					return Err(backoff::Error::permanent(io::Error::new(
						io::ErrorKind::Interrupted,
						"cancelled",
					)));
				}

				tracing::debug!(
					stream_id = %D::stream_id(),
					peer = %peer_addr.id,
					criteria = ?criteria,
					attempt = %attempt,
					"connecting to stream producer"
				);

				// attempt to establish a new connection to the remote producer
				let mut link = cancel
					.run_until_cancelled(connect_fut)
					.await
					.ok_or_else(cancelled_error)?
					.map_err(io::Error::other)?;

				// perform stream handshake to subscribe to the desired stream
				let handshake = ConsumerHandshake::new::<D>(criteria);
				cancel
					.run_until_cancelled(link.send_as(&handshake))
					.await
					.ok_or_else(cancelled_error)??;

				Ok(link)
			}
		})
		.await;

		match result {
			Ok(link) => {
				// successfully connected and performed handshake
				tracing::info!(
					stream_id = %D::stream_id(),
					peer = %self.peer.id(),
					criteria = ?self.criteria,
					attempts = %attempt,
					"connected to stream producer",
				);
				// set the receiver state to connected
				self.state_tx.send(State::Connected).ok();

				// prepare to receive the first datum
				let next_recv = self.make_next_recv_future(link);

				// begin listening for incoming data
				self.next_recv.set(next_recv);
			}
			Err(e) => {
				// explicitly cancelled through the cancellation token
				if e.kind() == io::ErrorKind::Interrupted {
					self.state_tx.send(State::Terminated).ok();
					return;
				}

				tracing::warn!(
					stream_id = %D::stream_id(),
					peer = %self.peer.id(),
					error = %e,
					criteria = ?self.criteria,
					attempts = %attempt,
					"Failed to connect to stream producer",
				);

				// unrecoverable error or cancelled, terminate the worker
				self.cancel.cancel();
				self.state_tx.send(State::Terminated).ok();
			}
		}
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
	/// A connection is being established with the remote producer.
	Connecting,

	/// A connection is established with the remote producer and it is actively
	/// receiving data.
	Connected,

	/// The connection with the remote producer is being gracefully closed.
	Terminating,

	/// The connection with the remote producer has been closed.
	/// Only once this state is reached this consumer may establish a new
	/// connection to the same producer.
	Terminated,
}

type StatusUpdatesStream =
	SelectAll<Pin<Box<dyn Stream<Item = (State, PeerId)> + Send>>>;
