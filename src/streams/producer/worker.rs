use {
	super::{
		super::{
			Criteria,
			Datum,
			NoCapacity,
			NotAllowed,
			StreamId,
			Streams,
			accept::StartStream,
		},
		Producer,
		Sinks,
		When,
		builder::ProducerConfig,
	},
	crate::{
		discovery::PeerEntry,
		network::link::{GracefulShutdown, Link},
		primitives::{Bytes, Short},
	},
	bincode::{config::standard, serde::encode_to_vec},
	core::{any::Any, cell::OnceCell},
	futures::future::join_all,
	slotmap::DenseSlotMap,
	std::sync::Arc,
	tokio::sync::{SetOnce, mpsc},
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
	accepted: mpsc::UnboundedSender<(Link<Streams>, Criteria, PeerEntry)>,

	/// A one-time set handle that is completed when the producer worker loop is
	/// initialized and ready to interact with other peers.
	ready: Arc<SetOnce<()>>,
}

impl Handle {
	/// Returns a typed sender channel for sending datum of type `D` to the
	/// sink worker loop.
	pub fn sender<D: Datum>(&self) -> Producer<D> {
		// Validate that the stream id matches the expected datum type
		assert_eq!(self.stream_id, D::stream_id(), "StreamId mismatch");

		// Downcast the type erased sender channel to the expected type
		let data_tx = self
			.data_tx
			.downcast_ref::<mpsc::Sender<D>>()
			.expect("Failed to downcast data sender channel");

		Producer::new(data_tx.clone(), When::new(self.ready.clone()))
	}

	/// Accepts an incoming connection from a remote consumer for this stream id.
	///
	/// By the time the connection is accepted, the [`Acceptor`] has already
	/// decoded the handshake message and opened a transport-level stream with
	/// the remote peer.
	pub fn accept(
		&self,
		link: Link<Streams>,
		criteria: Criteria,
		peer: PeerEntry,
	) {
		self
			.accepted
			.send((link, criteria, peer))
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
	/// Configuration for this producer sink.
	config: ProducerConfig,

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
	accepted: mpsc::UnboundedReceiver<(Link<Streams>, Criteria, PeerEntry)>,

	/// A one-time set handle that is completed when the producer worker loop is
	/// initialized and ready to interact with other peers.
	ready: Arc<SetOnce<()>>,
}

impl<D: Datum> WorkerLoop<D> {
	/// Spawns a new fanout sink worker loop for the given stream id.
	pub(super) fn spawn(sinks: &Sinks, config: ProducerConfig) -> Handle {
		let cancel = sinks.local.termination().child_token();

		let stream_id = D::stream_id();
		let ready = Arc::new(SetOnce::new());
		let (accepted_tx, accepted_rx) = mpsc::unbounded_channel();
		let (data_tx, data_rx) = mpsc::channel(config.buffer_size);

		let worker = WorkerLoop::<D> {
			config,
			cancel,
			data_rx,
			active: DenseSlotMap::with_key(),
			accepted: accepted_rx,
			ready: ready.clone(),
		};

		tokio::spawn(worker.run());

		Handle {
			stream_id,
			data_tx: Box::new(data_tx),
			accepted: accepted_tx,
			ready,
		}
	}
}

impl<D: Datum> WorkerLoop<D> {
	pub async fn run(mut self) {
		// mark the producer as ready after initial setup is done
		self.ready.set(()).expect("ready set once");

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
				Some((link, criteria, peer)) = self.accepted.recv() => {
					self.accept(link, criteria, peer).await;
				}

				// Triggered when a new datum is produced for this stream
				// by the public api via the [`Producer`] handle.
				Some(datum) = self.data_rx.recv() => {
					self.fanout(datum).await;
				}
			}
		}
	}

	/// Forwards the given datum to all active remote consumers that match the
	/// criteria.
	async fn fanout(&mut self, item: D) {
		let bytes = OnceCell::<Bytes>::new();
		let mut sends = Vec::with_capacity(self.active.len());
		for (sub_id, subscription) in &mut self.active {
			if subscription.criteria.matches(&item) {
				let bytes = bytes.get_or_init(|| {
					// Serialize the datum only once for all matching consumers,
					// if there is at least one consumer with criteria that matches.
					encode_to_vec(&item, standard())
						.expect("Failed to serialize datum")
						.into()
				});

				// SAFETY: `item` is properly serialized into `bytes` above.
				let send_fut = unsafe { subscription.link.send_raw(bytes.clone()) };
				sends.push(async move { (send_fut.await, sub_id) });
			}
		}

		// Await all send operations to complete
		let results = join_all(sends).await;
		for (res, _) in results {
			if let Err(reason) = res {
				tracing::warn!(
					stream_id = %Short(D::stream_id()),
					reason = %reason,
					"error sending datum to consumer",
				);
				// Remove the failed subscription
				todo!()
			}
		}
	}

	async fn accept(
		&mut self,
		link: Link<Streams>,
		criteria: Criteria,
		peer: PeerEntry,
	) {
		// Check if we have capacity to accept a new consumer
		if self.active.len() >= self.config.max_subscribers {
			tracing::debug!(
				consumer_id = %Short(&link.remote_id()),
				stream_id = %D::stream_id(),
				current_subscribers = %self.active.len(),
				"rejected new consumer: no capacity",
			);

			// Close the link with `NoCapacity` reason, this producer has
			// reached its maximum number of allowed subscribers.
			let _ = link.close(NoCapacity).await;
			return;
		}

		// Check if we should accept this consumer based on the auth predicate
		if !(self.config.accept_if)(&peer) {
			tracing::debug!(
				stream_id = %D::stream_id(),
				consumer = %Short(&peer),
				"rejected unauthorized consumer",
			);

			// Close the link with `NotAllowed` reason, this peer is not
			// authorized to subscribe to this stream.
			let _ = link.close(NotAllowed).await;
			return;
		}

		tracing::info!(
			consumer_id = %Short(&link.remote_id()),
			stream_id = %D::stream_id(),
			criteria = ?criteria,
			"accepted new consumer",
		);

		let mut link = link;
		let start_stream = StartStream(self.config.network_id, D::stream_id());

		if let Err(e) = link.send(&start_stream).await {
			tracing::error!(
				consumer_id = %Short(&link.remote_id()),
				error = %e,
				"failed to send start stream message to consumer",
			);
			return;
		}

		self.active.insert(Subscription {
			link,
			criteria,
			_peer: peer,
		});
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
			if let Err(reason) = subscription.link.close(GracefulShutdown).await
				&& !reason.is_cancelled()
			{
				tracing::debug!(
					stream_id = %Short(D::stream_id()),
					consumer_id = %Short(peer_id),
					reason = %reason,
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

	/// Snapshot of the remote consumer peer entry as known at the time of
	/// subscription creation.
	_peer: PeerEntry,
}
