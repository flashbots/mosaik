use {
	super::{
		super::{Config, Criteria, Datum, StreamId, Streams, accept::StartStream},
		Producer,
		Sinks,
		When,
	},
	crate::{
		network::link::{GracefulShutdown, Link},
		primitives::Short,
	},
	core::any::Any,
	slotmap::DenseSlotMap,
	std::sync::Arc,
	tokio::sync::mpsc,
	tokio_util::sync::CancellationToken,
};

/// Fanout Sink handle for a specific stream.
///
/// This structs provides an interface to interact with the long-running fanout
/// sink worker loop for a specific stream id.
pub(in crate::streams) struct Handle {
	/// The unique stream id associated with this sink.
	///
	/// This type is bound to the specific datum type `D` used to create the
	/// sink and is used to validate type parameters when casting the sender
	/// channel.
	stream_id: StreamId,

	/// A type erased sender channel for sending datum to the sink worker loop.
	///
	/// This is type erased to allow handles to be stored in a heterogeneous map
	/// such as `Sinks`.
	data_tx: Box<dyn Any + Send + Sync>,

	/// Channel for incoming accepted connections from remote consumers.
	/// This is populated by the [`Acceptor`] when a remote peer requests to
	/// subscribe to this stream and then passed to the worker loop to be added
	/// as a new subscription.
	accepted: mpsc::UnboundedSender<(Link<Streams>, Criteria)>,
}

impl Handle {
	/// Returns a typed sender channel for sending datum of type `D` to the
	/// sink worker loop.
	pub fn sender<D: Datum>(&self) -> Producer<D> {
		// Validate that the stream id matches the expected datum type
		assert_eq!(
			self.stream_id,
			D::stream_id(),
			"StreamId mismatch: expected {}, got {}",
			self.stream_id,
			D::stream_id()
		);

		// Downcast the type erased sender channel to the expected type
		let data_tx = self
			.data_tx
			.downcast_ref::<mpsc::Sender<D>>()
			.expect("Failed to downcast data sender channel");

		Producer::new(data_tx.clone(), When)
	}

	/// Accepts an incoming connection from a remote consumer for this stream id.
	///
	/// By the time the connection is accepted, the [`Acceptor`] has already
	/// decoded the handshake message and opened a transport-level stream with
	/// the remote peer.
	pub fn accept(&self, link: Link<Streams>, criteria: Criteria) {
		self
			.accepted
			.send((link, criteria))
			.expect("worker loop has shut down");
	}
}

/// Stream Fanout Sink worker loop
///
/// This is a long-running task that is owned by the [`Sinks`] struct that
/// manages all active fanout sinks for producers.
///
/// There is an instance of this struct for each active stream id that has
/// at least one producer associated with it.
pub(super) struct WorkerLoop<D: Datum> {
	/// Streams-wide configuration options of this node.
	config: Arc<Config>,

	/// Cancellation token triggered when the worker loop should shut down.
	/// This token is derived from the local node's termination token and will
	/// shut down when the node network is terminating.
	cancel: CancellationToken,

	/// Receiver channel for incoming datum to be forwarded to connected
	/// consumers.
	data_rx: mpsc::Receiver<D>,

	/// Active subscriptions from remote consumers for this stream each with
	/// potentially different criteria.
	active: DenseSlotMap<SubscriptionId, Subscription>,

	/// Incoming connections from remote consumers to be added as subscriptions.
	///
	/// Remote peers that arrive here are past the handshake phase and have an
	/// open transport-level stream.
	accepted: mpsc::UnboundedReceiver<(Link<Streams>, Criteria)>,
}

impl<D: Datum> WorkerLoop<D> {
	/// Spawns a new fanout sink worker loop for the given stream id.
	pub(super) fn spawn(sinks: &Sinks) -> Handle {
		let config = Arc::clone(&sinks.config);
		let cancel = sinks.local.termination().child_token();

		let stream_id = D::stream_id();
		let (accepted_tx, accepted_rx) = mpsc::unbounded_channel();
		let (data_tx, data_rx) = mpsc::channel(config.producer_buffer_size);

		let worker = WorkerLoop::<D> {
			config,
			cancel,
			data_rx,
			active: DenseSlotMap::with_key(),
			accepted: accepted_rx,
		};

		tokio::spawn(worker.run());

		Handle {
			stream_id,
			data_tx: Box::new(data_tx),
			accepted: accepted_tx,
		}
	}
}

impl<D: Datum> WorkerLoop<D> {
	pub async fn run(mut self) {
		loop {
			tokio::select! {
				// Triggered when the network is shutting down or
				// this stream is terminated due to all producers being dropped
				// or an unrecoverable error.
				() = self.cancel.cancelled() => {
					self.shutdown().await;
					break;
				}

				// Triggered when [`Acceptor`] accepts a new connection from a
				// remote consumer
				Some((link, criteria)) = self.accepted.recv() => {
					self.accept(link, criteria).await;
				}

				// Triggered when a new datum is produced for this stream
				// by the public api via the [`Producer`] handle.
				Some(datum) = self.data_rx.recv() => {
					self.fanout(datum);
				}
			}
		}
	}

	/// Forwards the given datum to all active remote consumers that match the
	/// criteria.
	fn fanout(&mut self, _datum: D) {
		tracing::warn!("implement producer fanout logic");
	}

	async fn accept(&mut self, link: Link<Streams>, criteria: Criteria) {
		tracing::info!(
			consumer_id = %Short(&link.remote_id()),
			stream_id = %D::stream_id(),
			criteria = ?criteria,
			"accepted new consumer",
		);

		let mut link = link;
		let start_stream = StartStream(D::stream_id());

		if let Err(e) = link.send(&start_stream).await {
			tracing::error!(
				consumer_id = %Short(&link.remote_id()),
				error = %e,
				"failed to send start stream message to consumer",
			);
			return;
		}

		let subscription = Subscription { link, criteria };
		self.active.insert(subscription);
	}

	/// Gracefully shuts down the worker loop by closing all active
	/// subscriptions.
	async fn shutdown(&mut self) {
		tracing::debug!(
			stream_id = %Short(D::stream_id()),
			"terminating stream producer",
		);

		for (_, subscription) in self.active.drain() {
			let peer_id = subscription.link.remote_id();
			if let Err(e) = subscription.link.close(GracefulShutdown).await {
				tracing::debug!(
					stream_id = %Short(D::stream_id()),
					consumer_id = %Short(peer_id),
					error = %e,
					"error closing subscription link",
				);
			}
		}
	}
}

slotmap::new_key_type! {
	/// A unique identifier for a remote subscription for one stream.
	///
	/// One remote node may have multiple subscriptions to the same stream
	/// with different criteria. Each subscription is identified by a unique
	/// [`SubscriptionId`] and is managed independently.
	struct SubscriptionId;
}

/// Represents an active subscription from a remote consumer.
/// Each subscription is an independent transport-level connection.
struct Subscription {
	/// The transport-level link to the remote consumer.
	link: Link<Streams>,

	/// The stream subscription criteria for this consumer.
	criteria: Criteria,
}
