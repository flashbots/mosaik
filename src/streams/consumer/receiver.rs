use {
	super::{
		super::{Config, Criteria, Streams},
		Datum,
	},
	crate::{
		discovery::PeerEntry,
		network::{
			LocalNode,
			link::{CloseReason, Link},
		},
		primitives::{Short, cancellable},
		streams::accept::ConsumerHandshake,
	},
	backoff::{backoff::Backoff, future::retry},
	core::{future::pending, ops::ControlFlow},
	futures::FutureExt,
	iroh::endpoint::ConnectionError,
	std::{io, sync::Arc},
	tokio::sync::{mpsc, watch},
	tokio_util::sync::{CancellationToken, ReusableBoxFuture},
};

/// Worker task that manages receiving data from one remote producer peer.
///
/// Notes:
///
/// - Individual receiver workers are spawned for each connected producer peer.
///
/// - A Producer will reject connections from consumers that are not known in
///   its discovery catalog. So if a consumer attempts to subscribe to a stream
///   from a producer and the connection is rejected with
///   [`CloseReason::UnknownPeer`] the consumer should trigger a catalog sync
///   with the producer and retry the subscription again.
pub(super) struct Receiver<D: Datum> {
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

	/// Backoff policy for reconnecting to the remote producer.
	backoff: Option<Box<dyn Backoff + Send + Sync + 'static>>,
}

/// Controls and observes the state of one stream receiver worker associated
/// with a remote producer.
///
/// There should be only one instance of this worker handle per remote
/// producer peer for a given consumer worker.
pub(super) struct ReceiverHandle {
	/// Cancellation token for terminating the receiver worker associated with
	/// this remote producer. This gets implicitly triggered when the parent
	/// consumer worker is dropped or the network is shutting down.
	cancel: CancellationToken,

	/// Observes changes to the receiver worker connection state.
	state: watch::Receiver<State>,
}

impl ReceiverHandle {
	/// Terminates the receiver worker and waits for it to shut down.
	pub async fn terminate(mut self) {
		// Fire off the cancellation signal
		self.cancel.cancel();

		// Wait for the worker to observe the cancellation and set its state to
		// `Terminated`.
		let _ = self.state.wait_for(|s| *s == State::Terminated).await;
	}

	/// Returns a watch handle for monitoring the receiver state.
	pub const fn state(&self) -> &watch::Receiver<State> {
		&self.state
	}
}

impl<D: Datum> Receiver<D> {
	pub fn spawn(
		peer: PeerEntry,
		local: &LocalNode,
		cancel: &CancellationToken,
		data_tx: &mpsc::UnboundedSender<D>,
		config: &Arc<Config>,
		criteria: &Criteria,
	) -> ReceiverHandle {
		let local = local.clone();
		let cancel = cancel.child_token();
		let data_tx = data_tx.clone();
		let config = Arc::clone(config);
		let criteria = criteria.clone();
		let next_recv = ReusableBoxFuture::new(pending());
		let (state_tx, state) = watch::channel(State::Connecting);

		let worker = Receiver {
			config,
			local,
			peer,
			criteria,
			data_tx,
			state_tx,
			cancel: cancel.clone(),
			next_recv,
			backoff: None,
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
					let _ = self.state_tx
						.subscribe()
						.wait_for(|s| *s == State::Terminated)
						.await;

					break;
				}

				// Triggered when new data is received from the remote producer
				// this will enqueue the next receive future.
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
					// reset the global backoff policy on successful receive
					if let Some(ref mut backoff) = self.backoff {
						backoff.reset();
					}

					self.next_recv.set(self.make_next_recv_future(link));
				}
			}
			// an error occurred while receiving the datum,
			// unless cancelled, drop the current link and attempt to
			// reconnect according to the backoff policy.
			Err(e) => {
				match (e.kind(), link.connection().close_reason()) {
					(io::ErrorKind::Interrupted, _) => {
						// explicitly cancelled through the cancellation token
						let _ = link.close_with_reason(CloseReason::Unspecified).await;
						self.state_tx.send(State::Terminated).ok();
						return;
					}
					// The connection was closed by the producer because it does not have
					// this consumer in its discovery catalog. Trigger full catalog
					// sync with the producer.
					(_, Some(ConnectionError::ApplicationClosed(reason)))
						if reason == CloseReason::UnknownPeer =>
					{
						self.sync_catalog_then_reconnect(link);
						return;
					}
					_ => {
						// io error occurred, drop the current link and attempt to reconnect
						tracing::warn!(
							error = %e,
							stream_id = %D::stream_id(),
							producer_id = %Short(self.peer.id()),
						);
					}
				}

				self.connect(Some(link)).await;
			}
		}
	}

	/// Creates a future that receives the next datum from the remote producer
	/// over the specified link.
	///
	/// The future is cancellable using the worker's cancellation token and
	/// carries the link along with it for further receives or reconnections.
	///
	/// The first instance of this future is created by [`connect`].
	fn make_next_recv_future(
		&self,
		mut link: Link,
	) -> impl Future<Output = (Result<D, io::Error>, Link)> + 'static {
		let cancel = self.cancel.clone();

		// bind the the receive future along with the link instance for next
		// receive polls or for connection recovery logic.
		async move { (cancellable(&cancel, link.recv_as::<D>()).await, link) }
			.fuse()
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

		// apply backoff before attempting to reconnect
		if self.apply_backoff().await.is_break() {
			return;
		}

		let criteria = &self.criteria;
		let peer_addr = self.peer.address();
		let backoff = (self.config.backoff)();

		// create a connect-and-handshake future with retries according to
		// the backoff policy
		let result: Result<Link, _> = retry(backoff, || {
			let criteria = criteria.clone();
			let cancel = self.cancel.clone();
			let connect_fut = self.local.connect(peer_addr.clone(), Streams::ALPN);

			async move {
				tracing::debug!(
					stream_id = %D::stream_id(),
					producer_id = %Short(&peer_addr.id),
					criteria = ?criteria,
					"connecting to stream producer",
				);

				// attempt to establish a new connection to the remote producer
				let mut link = cancellable(&cancel, connect_fut).await?;

				// Send the consumer handshake to the producer
				let handshake = ConsumerHandshake::new::<D>(criteria);
				cancellable(&cancel, link.send_as(&handshake)).await?;

				Ok(link)
			}
		})
		.await;

		match result {
			Ok(link) => {
				// successfully connected and performed handshake
				tracing::info!(
					stream_id = %D::stream_id(),
					producer_id = %Short(&self.peer.id()),
					criteria = ?self.criteria,
					"connected to stream producer",
				);
				// set the receiver state to connected
				self.state_tx.send(State::Connected).ok();

				// begin listening for incoming data
				self.next_recv.set(self.make_next_recv_future(link));
			}
			Err(e) => {
				if !self.cancel.is_cancelled() {
					tracing::error!(
						error = %e,
						stream_id = %D::stream_id(),
						producer_id = %Short(&self.peer.id()),
						criteria = ?self.criteria,
					  "failed to connect to stream producer");
					self.cancel.cancel();
				}
				// no more cleanup to do, mark the worker as terminated
				self.state_tx.send(State::Terminated).ok();
			}
		}
	}

	fn sync_catalog_then_reconnect(&mut self, link: Link) {
		tracing::info!(
			".._> syncing catalog with producer before reconnecting for link {link}"
		);
	}

	async fn apply_backoff(&mut self) -> ControlFlow<()> {
		match self.backoff {
			None => {
				self.backoff = Some((self.config.backoff)());
				ControlFlow::Continue(())
			}
			Some(ref mut backoff) => {
				let Some(duration) = backoff.next_backoff() else {
					// backoff policy has been exhausted, terminate the worker
					tracing::debug!(
						stream_id = %D::stream_id(),
						producer_id = %Short(&self.peer.id()),
						criteria = ?self.criteria,
						"exhausted all reconnection attempts, terminating",
					);

					self.cancel.cancel();
					self.state_tx.send(State::Terminated).ok();

					return ControlFlow::Break(());
				};

				tracing::debug!(
					stream_id = %D::stream_id(),
					producer_id = %Short(&self.peer.id()),
					criteria = ?self.criteria,
					"waiting {duration:?} before reconnecting",
				);

				tokio::time::sleep(duration).await;
				ControlFlow::Continue(())
			}
		}
	}
}

/// The current connection state of a stream receiver worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum State {
	/// A connection is being established with the remote producer.
	Connecting,

	/// A connection is established with the remote producer and it is actively
	/// receiving data.
	Connected,

	/// The connection with the remote producer has been closed.
	/// Only once this state is reached this consumer may establish a new
	/// connection to the same producer.
	Terminated,
}
