use {
	super::super::{
		StreamNotFound,
		Streams,
		UnknownPeer,
		accept::{ConsumerHandshake, StartStream},
		consumer::builder::ConsumerConfig,
		status::{State, Stats},
	},
	crate::{
		Datum,
		discovery::{Discovery, PeerEntry},
		network::{
			LocalNode,
			link::{DifferentNetwork, GracefulShutdown, Link, LinkError, RecvError},
		},
		primitives::Short,
		streams::status::ChannelInfo,
	},
	backoff::backoff::Backoff,
	core::{future::pending, ops::ControlFlow},
	futures::{FutureExt, TryFutureExt},
	std::sync::Arc,
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
	/// Configuration for this consumer.
	config: Arc<ConsumerConfig>,

	/// Discovery system handle used to trigger catalog syncs with producers that
	/// are not recognizing this consumer.
	discovery: Discovery,

	/// Local socket, used to initiate connections to remote peers on the Streams
	/// protocol.
	local: LocalNode,

	/// The remote producer peer entry snapshot as known at the time of worker
	/// creation.
	peer: Arc<PeerEntry>,

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
	next_recv: ReusableBoxFuture<'static, (DatumRecvResult<D>, Link<Streams>)>,

	/// Backoff policy for reconnecting to the remote producer on recoverable
	/// errors that is currently being applied since the last successful receive.
	backoff: Option<Box<dyn Backoff + Send + Sync + 'static>>,

	/// Statistics about this receiver worker.
	stats: Arc<Stats>,
}

impl<D: Datum> Receiver<D> {
	/// Spawns a new receiver worker for the specified remote producer peer.
	///
	/// An instance of this worker should be created for each connected producer
	/// peer for a consumer instance.
	pub fn spawn(
		peer: PeerEntry,
		local: &LocalNode,
		discovery: &Discovery,
		cancel: &CancellationToken,
		data_tx: &mpsc::UnboundedSender<D>,
		config: Arc<ConsumerConfig>,
	) -> ChannelInfo {
		let local = local.clone();
		let cancel = cancel.child_token();
		let data_tx = data_tx.clone();
		let discovery = discovery.clone();
		let peer = Arc::new(peer);
		let stats = Arc::new(Stats::default());
		let next_recv = ReusableBoxFuture::new(pending());
		let (state_tx, state) = watch::channel(State::Connecting);

		// Construct the status signaling info for this receiver worker.
		let channel_info = ChannelInfo {
			stream_id: D::stream_id(),
			criteria: config.criteria.clone(),
			producer_id: *peer.id(),
			consumer_id: local.id(),
			stats: Arc::clone(&stats),
			peer: Arc::clone(&peer),
			state: state.clone(),
		};

		let worker = Receiver {
			local,
			discovery,
			data_tx,
			state_tx,
			next_recv,
			backoff: None,
			cancel: cancel.child_token(),
			peer: Arc::clone(&peer),
			stats: Arc::clone(&stats),
			config,
		};

		tokio::spawn(worker.run());

		channel_info
	}
}

impl<D: Datum> Receiver<D> {
	pub async fn run(mut self) {
		// initial connection attempt
		self.connect().await;

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

				// Triggered when the next datum read future is ready.
				// This could be either a successfully received datum or an error.
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
	async fn on_next_recv(
		&mut self,
		result: DatumRecvResult<D>,
		link: Link<Streams>,
	) {
		match result {
			// a datum was successfully received
			Ok((datum, bytes_len)) => {
				// forward the received datum to the consumer worker
				// for delivery to public api consumer handle.
				self.data_tx.send(datum).ok();

				// update stats
				self.stats.increment_datums();
				self.stats.increment_bytes(bytes_len);

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
			// kick off the sad path handling for the current link.
			Err(error) => self.handle_recv_error(error).await,
		}
	}

	/// Creates a future that receives the next datum from the remote producer
	/// over the specified link.
	///
	/// The future is cancellable using the worker's cancellation token and
	/// carries the link along with it for further receives or reconnections.
	///
	/// The first instance of this future is created by [`connect`].
	#[expect(clippy::unused_self)]
	fn make_next_recv_future(
		&self,
		mut link: Link<Streams>,
	) -> impl Future<Output = (Result<(D, usize), LinkError>, Link<Streams>)> + 'static
	{
		// bind the the receive future along with the transport link it is using so
		// we can repair the connection if needed.

		async move {
			let fut = link.recv_with_size::<D>().map_err(LinkError::Recv);
			(fut.await, link)
		}
		.fuse()
	}

	/// Attempts to connect to the remote producer and perform the stream
	/// subscription handshake. This method will retry connections according
	/// to the backoff policy specified in the configuration.
	async fn connect(&mut self) {
		self.state_tx.send(State::Connecting).ok();

		// apply backoff before attempting to reconnect
		if self.apply_backoff().await.is_break() {
			return;
		}

		let cancel = &self.cancel;
		let criteria = self.config.criteria.clone();
		let peer_addr = self.peer.address();

		let result = async {
			tracing::debug!(
				stream_id = %D::stream_id(),
				producer_id = %Short(&peer_addr.id),
				criteria = ?criteria,
				"connecting to stream producer",
			);

			// attempt to establish a new connection to the remote producer
			let mut link = self
				.local
				.connect_with_cancel::<Streams>(peer_addr.clone(), cancel.clone())
				.await?;

			// Send the consumer handshake to the producer
			link
				.send(&ConsumerHandshake::new::<D>(
					*self.local.network_id(),
					criteria,
				))
				.await?;

			// await the producer's handshake response
			let start = link.recv::<StartStream>().await?;

			// confirm that the producer is on the correct network
			if start.network_id() != *self.local.network_id() {
				tracing::warn!(
					stream_id = %D::stream_id(),
					producer_id = %Short(&peer_addr.id),
					expected_network = %Short(self.local.network_id()),
					received_network = %Short(start.network_id()),
					"producer is on a different network",
				);
				return Err(LinkError::Recv(RecvError::closed(DifferentNetwork)));
			}

			// confirm that the producer is producing the requested stream
			if start.stream_id() != D::stream_id() {
				tracing::warn!(
					stream_id = %D::stream_id(),
					producer_id = %Short(&peer_addr.id),
					"producer is producing a different stream than requested",
				);
				return Err(LinkError::Recv(RecvError::closed(StreamNotFound)));
			}

			Ok(link)
		};

		match result.await {
			Ok(link) => {
				// successfully connected and performed handshake
				tracing::info!(
					stream_id = %D::stream_id(),
					producer_id = %Short(&self.peer.id()),
					criteria = ?self.config.criteria,
					"connected to stream producer",
				);

				// set the receiver state to connected
				self.state_tx.send(State::Connected).ok();

				// update stats
				self.stats.connected();

				// begin listening for incoming data
				self.next_recv.set(self.make_next_recv_future(link));
			}
			Err(error) => self.handle_recv_error(error).await,
		}
	}

	/// Handles errors that occur while receiving data from the remote producer or
	/// during initial connection setup.
	///
	/// Application-level errors such as being unknown to the producer are
	/// communicated by the producer by closing the connection with a specific
	/// [`CloseReason`].
	///
	/// Any error during receiving data will result in dropping the current link
	/// (if it was not already closed by the producer) and potentially repairing
	/// with a new connection according to the backoff policy.
	///
	/// Inside this method, if the error is unrecoverable, the worker's
	/// cancellation token is triggered to initiate shutdown.
	async fn handle_recv_error(&mut self, error: LinkError) {
		let close_reason = error.close_reason().cloned();

		// indicates an unrecoverable error that should terminate the worker
		// and not attempt to repair the connection any further.
		macro_rules! unrecoverable {
			() => {
				self.cancel.cancel();
				self.state_tx.send(State::Terminated).ok();
				self.stats.disconnected();
				return;
			};

			($msg:expr, $e:expr) => {
				tracing::warn!(
					stream_id = %D::stream_id(),
					producer_id = %Short(&self.peer.id()),
					criteria = ?self.config.criteria,
					error = %$e,
					$msg,
				);

				self.cancel.cancel();
				self.state_tx.send(State::Terminated).ok();
				self.stats.disconnected();
				return;
			};
		}

		match (error, close_reason) {
			// Consumer or network is terminating
			(LinkError::Cancelled, _) => {
				// explicitly cancelled through the cancellation token, shut down
				unrecoverable!();
			}

			// Received datum could not be deserialized
			(LinkError::Recv(RecvError::Decode(err)), _) => {
				// High likelihood of malicious or buggy producer sending invalid data.
				unrecoverable!("producer sent invalid datum", err);
			}

			// The connection was closed by the producer because it does not have
			// this consumer in its discovery catalog. Trigger full catalog
			// sync with the producer then reconnect.
			(_, Some(reason)) if reason == UnknownPeer => {
				tracing::debug!(
					stream_id = %D::stream_id(),
					producer_id = %Short(&self.peer.id()),
					"producer does not recognize this consumer",
				);

				let _ = self
					.discovery
					.sync_with(self.peer.address().clone())
					.await
					.inspect_err(|e| {
						tracing::warn!(
							error = %e,
							stream_id = %D::stream_id(),
							producer_id = %Short(&self.peer.id()),
							"failed to sync catalog with producer",
						);
					});
			}
			// the connection was closed gracefully by the producer because it is
			// shutting down. Don't attempt to reconnect.
			(_, Some(reason)) if reason == GracefulShutdown => {
				unrecoverable!("producer is shutting down", "none");
			}
			(e, Some(reason)) if reason == StreamNotFound => {
				// the reason why we are not reconnecting on this error is because
				// producers are discovered through the discovery catalog which
				// should only list producers that are actually producing the
				// requested stream. If we reach this point it indicates a bug
				// either in the discovery system or the producer's stream
				// registration logic.
				// todo: Revisit this decision later.
				unrecoverable!("producer does not have the requested stream", e);
			}
			(e, Some(reason)) if reason == DifferentNetwork => {
				// The producer is on a different network than this consumer.
				// This is unrecoverable and most likely indicates a corrupt or stale
				// discovery catalog.
				unrecoverable!("producer is on a different network", e);
			}
			(_, Some(reason)) => {
				// The producer closed the connection with an application-level
				// error that is not explicitly handled above and is not known
				// to be unrecoverable. Log and attempt to reconnect.
				tracing::warn!(
					stream_id = %D::stream_id(),
					producer_id = %Short(&self.peer.id()),
					reason = %reason,
					"subscription refused by producer",
				);
			}
			(e, _) => {
				// io error occurred, drop the current link and attempt to reconnect
				tracing::warn!(
					error = %e,
					stream_id = %D::stream_id(),
					producer_id = %Short(self.peer.id()),
				);
			}
		}

		Box::pin(self.connect()).await;
	}

	/// Applies the backoff policy before attempting to reconnect to the
	/// remote producer. The backoff policy is reset on successful receives and
	/// initialized to the default starting state on the first connection attempt.
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
						criteria = ?self.config.criteria,
						"exhausted all reconnection attempts, terminating",
					);

					self.cancel.cancel();
					self.state_tx.send(State::Terminated).ok();

					return ControlFlow::Break(());
				};

				tracing::debug!(
					stream_id = %D::stream_id(),
					producer_id = %Short(&self.peer.id()),
					criteria = ?self.config.criteria,
					"waiting {duration:?} before reconnecting",
				);

				tokio::time::sleep(duration).await;
				ControlFlow::Continue(())
			}
		}
	}
}

type DatumRecvResult<D> = Result<(D, usize), LinkError>;
