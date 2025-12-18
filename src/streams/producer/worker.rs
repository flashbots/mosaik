use {
	super::{
		super::{
			Criteria,
			Datum,
			NoCapacity,
			NotAllowed,
			Streams,
			accept::StartStream,
			status::ChannelInfo,
		},
		Producer,
		Sinks,
		When,
		builder::ProducerConfig,
	},
	crate::{
		PeerId,
		discovery::PeerEntry,
		network::link::{GracefulShutdown, Link},
		primitives::{Bytes, Short, UniqueId},
		streams::status::{ActiveChannelsMap, ChannelConditions, State, Stats},
	},
	bincode::{config::standard, serde::encode_to_vec},
	core::{any::Any, cell::OnceCell, pin::Pin},
	futures::{FutureExt, StreamExt, future::join_all, stream::FuturesUnordered},
	iroh::endpoint::ConnectionError,
	slotmap::DenseSlotMap,
	std::sync::Arc,
	tokio::sync::{mpsc, watch},
	tokio_util::sync::CancellationToken,
};

/// Fanout Sink handle for a specific stream.
///
/// This structs provides an interface to interact with the long-running
/// producer fanout sink worker loop for a specific stream id.
pub(in crate::streams) struct Handle {
	/// The configuration used to create this producer sink.
	config: Arc<ProducerConfig>,

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

	/// Observer for the ready status of the producer.
	ready: watch::Receiver<bool>,

	/// Observer for the active subscriptions info map.
	active: watch::Receiver<ActiveChannelsMap>,
}

impl Handle {
	/// Returns a typed sender channel for sending datum of type `D` to the
	/// sink worker loop.
	pub fn sender<D: Datum>(&self) -> Producer<D> {
		// Downcast the type erased sender channel to the expected type
		let data_tx = self
			.data_tx
			.downcast_ref::<mpsc::Sender<D>>()
			.expect("datum type mismatch; this is a bug.");

		Producer::new(
			data_tx.clone(),
			When::new(self.active.clone(), self.ready.clone()),
			Arc::clone(&self.config),
		)
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
	/// The local node's peer id.
	local_id: PeerId,

	/// Configuration for this producer sink.
	config: Arc<ProducerConfig>,

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

	/// Channel for broadcasting active subscription info to observers.
	///
	/// This is used by public status APIs to get the current snapshot of
	/// active subscriptions to this producer.
	active_info: watch::Sender<ActiveChannelsMap>,

	/// Incoming connections from remote consumers to be added as subscriptions.
	///
	/// Remote peers that arrive here are past the handshake phase and have an
	/// open transport-level stream.
	accepted: mpsc::UnboundedReceiver<(Link<Streams>, Criteria, PeerEntry)>,

	/// Futures that resolve when a remote consumer connection is dropped for
	/// whatever reason.
	dropped: FuturesUnordered<DroppedFuture>,

	/// Sets the online status of the producer when the worker loop is ready and
	/// publishing conditions are met.
	online: watch::Sender<bool>,

	/// A future that resolves when the producer can publish data based on
	/// the configured online conditions.
	online_when: ChannelConditions,
}

impl<D: Datum> WorkerLoop<D> {
	/// Spawns a new fanout sink worker loop for the given stream id.
	pub(super) fn spawn(sinks: &Sinks, config: ProducerConfig) -> Handle {
		let cancel = sinks.local.termination().child_token();

		let online = watch::Sender::new(false);
		let config = Arc::new(config);
		let active_info = watch::Sender::new(im::HashMap::new());
		let (accepted_tx, accepted_rx) = mpsc::unbounded_channel();
		let (data_tx, data_rx) = mpsc::channel(config.buffer_size);
		let online_when = (config.online_when)(
			When::new(active_info.subscribe(), online.subscribe()).subscribed(),
		);
		online.send_replace(online_when.is_condition_met());

		let worker = WorkerLoop::<D> {
			cancel,
			data_rx,
			local_id: sinks.local.id(),
			config: Arc::clone(&config),
			active: DenseSlotMap::with_key(),
			accepted: accepted_rx,
			online: online.clone(),
			dropped: FuturesUnordered::new(),
			active_info: active_info.clone(),
			online_when,
		};

		tokio::spawn(worker.run());

		tracing::info!(
			stream_id = %Short(config.stream_id),
			network_id = %config.network_id,
			"created new stream producer",
		);

		Handle {
			config,
			data_tx: Box::new(data_tx),
			accepted: accepted_tx,
			ready: online.subscribe(),
			active: active_info.subscribe(),
		}
	}
}

impl<D: Datum> WorkerLoop<D> {
	pub async fn run(mut self) {
		let mut offline_when = self.online_when.clone().unmet();

		loop {
			tokio::select! {
				// Triggered when the network is shutting down or
				// this stream is terminated due to all producers being dropped
				// or an unrecoverable error.
				() = self.cancel.cancelled() => {
					self.shutdown().await;
					break;
				}

				// Triggered when the publishing conditions for this producer
				// are met and it is considered online.
				() = &mut self.online_when => {
					self.on_online();
				}

				// Triggered when the publishing conditions for this producer
				// are no longer met and it is considered offline.
				() = &mut offline_when => {
					self.on_offline();
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

				// Triggered when any of the active remote consumer connections
				// is dropped for any reason.
				Some((sub_id, reason)) = self.dropped.next() => {
					self.on_connection_dropped(sub_id, &reason);
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
						.expect("failed to serialize datum")
						.into()
				});

				// SAFETY: `item` is properly serialized into `bytes` above.
				let send_fut = unsafe { subscription.link.send_raw(bytes.clone()) };
				sends.push(async move { (send_fut.await, sub_id) });

				// Update statistics (todo)
				subscription.stats.increment_datums();
				subscription.stats.increment_bytes(bytes.len());
			}
		}

		// Await all send operations to complete
		let results = join_all(sends).await;
		for (res, _) in results {
			if let Err(reason) = res {
				tracing::warn!(
					stream_id = %Short(self.config.stream_id),
					reason = %reason,
					"error sending datum to consumer",
				);
				// Remove the failed subscription
				todo!("handle failed sends properly");
			}
		}
	}

	/// Called by the protocol acceptor when a new remote consumer connection
	/// is opened for this stream. This is responsible for validating the
	/// connection and adding it to the active subscriptions if accepted.
	///
	/// Upon successful acceptance, a `StartStream` message is sent to the remote
	/// consumer to initiate the stream.
	async fn accept(
		&mut self,
		link: Link<Streams>,
		criteria: Criteria,
		peer: PeerEntry,
	) {
		// Check if we have capacity to accept a new consumer
		if self.active.len() >= self.config.max_subscribers {
			tracing::warn!(
				consumer_id = %Short(&link.remote_id()),
				stream_id = %Short(self.config.stream_id),
				current_subscribers = %self.active.len(),
				"rejected consumer connection: no capacity",
			);

			// Close the link with `NoCapacity` reason, this producer has
			// reached its maximum number of allowed subscribers.
			let _ = link.close(NoCapacity).await;
			return;
		}

		// Check if we should accept this consumer based on the auth predicate
		if !(self.config.accept_if)(&peer) {
			tracing::warn!(
				stream_id = %Short(self.config.stream_id),
				consumer_id = %Short(&peer),
				"rejected unauthorized consumer",
			);

			// Close the link with `NotAllowed` reason, this peer is not
			// authorized to subscribe to this stream.
			let _ = link.close(NotAllowed).await;
			return;
		}

		let mut link = link;
		let start_stream =
			StartStream(self.config.network_id, self.config.stream_id);

		if let Err(e) = link.send(&start_stream).await {
			tracing::warn!(
				consumer_id = %Short(&link.remote_id()),
				error = %e,
				"failed to confirm stream subscription with consumer",
			);
			return;
		}

		let peer = Arc::new(peer);
		let stats = Arc::new(Stats::default());
		let state = watch::Sender::new(State::Connected);
		stats.connected(); // mark connection timestamp

		tracing::info!(
			consumer_id = %Short(&link.remote_id()),
			stream_id = %Short(self.config.stream_id),
			criteria = ?criteria,
			total_consumers = %self.active.len() + 1,
			"accepted new consumer",
		);

		// Add the subscription to the active list
		let sub_id = self.active.insert(Subscription {
			link,
			criteria: criteria.clone(),
			stats: Arc::clone(&stats),
			state: state.clone(),
		});

		// Track when the remote consumer connection is dropped for any reason
		let fut = self.active[sub_id].link.disconnected();
		self.dropped.push(fut.map(move |r| (sub_id, r)).boxed());

		// Publish an updated snapshot of active subscriptions info
		// for public status APIs.
		self.active_info.send_modify(|active| {
			let sub_id = UniqueId::from_u64(sub_id.0.as_ffi());
			active.insert(sub_id, ChannelInfo {
				consumer_id: *peer.id(),
				producer_id: self.local_id,
				stream_id: self.config.stream_id,
				state: state.subscribe(),
				peer,
				stats,
				criteria,
			});
		});
	}

	/// Gracefully shuts down the worker loop by closing all active
	/// subscriptions.
	async fn shutdown(&mut self) {
		tracing::debug!(
			stream_id = %Short(self.config.stream_id),
			"terminating stream producer",
		);

		for (sub_id, subscription) in self.active.drain() {
			// Remove subscription from the active info map and signal
			// to status observers that the subscription is gone.
			self.active_info.send_modify(|active| {
				let sub_id = UniqueId::from_u64(sub_id.0.as_ffi());
				active.remove(&sub_id);
			});

			subscription.stats.disconnected();
			let _ = subscription.state.send(State::Terminated);

			let peer_id = subscription.link.remote_id();
			if let Err(reason) = subscription.link.close(GracefulShutdown).await
				&& !reason.is_cancelled()
			{
				tracing::debug!(
					stream_id = %Short(self.config.stream_id),
					consumer_id = %Short(peer_id),
					reason = %reason,
					"error closing subscription link",
				);
			}
		}
	}

	/// Handles a dropped connection from a remote consumer.
	fn on_connection_dropped(
		&mut self,
		sub_id: SubscriptionId,
		reason: &ConnectionError,
	) {
		if let Some(subscription) = self.active.remove(sub_id) {
			subscription.stats.disconnected();
			let _ = subscription.state.send(State::Terminated);

			// Remove subscription from the active info map and signal
			// to status observers that the subscription is gone.
			self.active_info.send_modify(|active| {
				let sub_id = UniqueId::from_u64(sub_id.0.as_ffi());
				active.remove(&sub_id);
			});

			tracing::info!(
				reason = %reason,
				stream_id = %Short(self.config.stream_id),
				consumer_id = %Short(&subscription.link.remote_id()),
				remaining_consumers = %self.active.len(),
				"consumer disconnected",
			);
		}
	}

	fn on_online(&mut self) {
		tracing::trace!(
			stream_id = %Short(self.config.stream_id),
			consumers = %self.active.len(),
			"producer is online",
		);

		self.online.send_replace(true);
	}

	fn on_offline(&mut self) {
		tracing::trace!(
			stream_id = %Short(self.config.stream_id),
			consumers = %self.active.len(),
			"producer is offline",
		);

		self.online.send_replace(false);
	}
}

slotmap::new_key_type! {
	/// A unique identifier for a remote subscription for one stream.
	///
	/// One remote node may have multiple subscriptions to the same stream
	/// with different criteria. Each subscription is identified by a unique
	/// [`SubscriptionId`] and is managed independently.
	pub(crate) struct SubscriptionId;
}

/// Represents an active subscription from a remote consumer.
/// Each subscription is an independent transport-level connection.
struct Subscription {
	/// The transport-level link to the remote consumer.
	link: Link<Streams>,

	/// The stream subscription criteria for this consumer.
	criteria: Criteria,

	/// Statistics tracker for this subscription.
	stats: Arc<Stats>,

	/// State watcher for this subscription.
	state: watch::Sender<State>,
}

type DroppedFuture = Pin<
	Box<dyn Future<Output = (SubscriptionId, ConnectionError)> + Send + 'static>,
>;
