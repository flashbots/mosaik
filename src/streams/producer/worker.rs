use {
	super::{
		super::{Config, Criteria, Datum, StreamId},
		Producer,
		Sinks,
		Status,
	},
	crate::network::link::Link,
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
	accepted: mpsc::UnboundedSender<(Link, Criteria)>,
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

		Producer::new(data_tx.clone(), Status)
	}

	/// Accepts an incoming connection from a remote consumer for this stream id.
	///
	/// By the time the connection is accepted, the [`Acceptor`] has already
	/// decoded the handshake message and opened a transport-level stream with
	/// the remote peer.
	pub fn accept(&self, link: Link, criteria: Criteria) {
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
	accepted: mpsc::UnboundedReceiver<(Link, Criteria)>,

	/// Cancellation token triggered when the worker loop should shut down.
	/// This token is derived from the local node's termination token and will
	/// shut down when the node network is terminating.
	cancel: CancellationToken,
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
			data_rx,
			active: DenseSlotMap::with_key(),
			accepted: accepted_rx,
			cancel,
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
					tracing::info!(
						stream_id = %D::stream_id(),
						"Shutting down stream fanout sink worker",
					);
					break;
				}

				// Triggered when [`Acceptor`] accepts a new connection from a
				// remote consumer
				Some((link, criteria)) = self.accepted.recv() => {
					tracing::info!(
						consumer_id = %link.remote_id(),
						stream_id = %D::stream_id(),
						criteria = ?criteria,
						"accepting new consumer",
					);
				}
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
	link: Link,

	/// The stream subscription criteria for this consumer.
	criteria: Criteria,

	/// Connection Id of the remote consumer.
	subid: SubscriptionId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
	Connected,
	Terminating,
	Terminated,
}
