use {
	super::{
		super::{Criteria, Datum, NoCapacity, NotAllowed, Streams},
		Producer,
		Sinks,
		When,
		builder::ProducerConfig,
		sender::{Sender, Subscription},
	},
	crate::{
		PeerId,
		discovery::{
			Catalog,
			PeerEntry,
			rtt::{PeerInfo, RttTracker},
		},
		network::{GracefulShutdown, link::Link},
		primitives::{Bytes, Digest, Short, ShortFmtExt},
		streams::{
			TooSlow,
			status::{ActiveChannelsMap, ChannelConditions},
		},
	},
	core::{any::Any, cell::OnceCell},
	futures::FutureExt,
	slotmap::DenseSlotMap,
	std::sync::Arc,
	tokio::{
		sync::{mpsc, watch},
		task::JoinSet,
	},
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

	/// Observer for the status of active subscriptions to this producer.
	when: When,
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

		Producer::new(data_tx.clone(), self.when.clone(), Arc::clone(&self.config))
	}

	/// Accepts an incoming connection from a remote consumer for this stream id.
	///
	/// By the time the connection is accepted, the [`Acceptor`] has already
	/// decoded the handshake message and opened a transport-level stream with
	/// the remote peer.
	#[expect(clippy::result_large_err)]
	pub fn accept(
		&self,
		link: Link<Streams>,
		criteria: Criteria,
		peer: PeerEntry,
	) -> Result<(), Link<Streams>> {
		self
			.accepted
			.send((link, criteria, peer))
			.map_err(|mpsc::error::SendError((link, _, _))| link)
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

	/// RTT tracker for recording connection latency samples.
	rtt: Arc<RttTracker>,

	/// Cancellation token triggered when the worker loop should shut down.
	/// This token is derived from the local node's termination token and will
	/// shut down when the node network is terminating.
	cancel: CancellationToken,

	/// Receiver channel for incoming datum to be forwarded to connected
	/// consumers.
	data_rx: mpsc::Receiver<D>,

	/// Active subscriptions from remote consumers.
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
	dropped: JoinSet<SubscriptionId>,

	/// Futures that resolve when a consumer's ticket expires.
	ticket_expiries: JoinSet<SubscriptionId>,

	/// Watch receiver for discovery catalog changes. When the catalog
	/// updates (e.g. periodic peer announcements), `require()`
	/// predicates are re-evaluated against active consumers so that
	/// RTT-based requirements are enforced after paths settle.
	catalog: watch::Receiver<Catalog>,

	/// Sets the online status of the producer when the worker loop is ready and
	/// publishing conditions are met.
	online: watch::Sender<bool>,

	/// A future that resolves when the producer can publish data based on
	/// the configured online conditions.
	online_when: ChannelConditions,

	/// Pre-computed metrics labels for this stream.
	metrics_labels: [(&'static str, String); 2],
}

impl<D: Datum> WorkerLoop<D> {
	/// Spawns a new fanout sink worker loop for the given stream id.
	pub(super) fn spawn(sinks: &Sinks, config: ProducerConfig) -> Handle {
		let cancel = sinks.local.termination().child_token();

		let config = Arc::new(config);
		let online = watch::Sender::new(false);
		let active_info = watch::Sender::new(im::HashMap::new());
		let when = When::new(active_info.subscribe(), online.subscribe());
		let online_when = (config.online_when)(when.subscribed());
		let (accepted_tx, accepted_rx) = mpsc::unbounded_channel();
		let (data_tx, data_rx) = mpsc::channel(config.buffer_size);

		online.send_replace(online_when.is_condition_met());

		let rtt = Arc::clone(sinks.discovery.rtt_tracker());
		let catalog = sinks.discovery.catalog_watch();

		let metrics_labels = [
			("stream", config.stream_id.short().to_string()),
			("network", config.network_id.short().to_string()),
		];

		let worker = Self {
			cancel,
			data_rx,
			local_id: sinks.local.id(),
			rtt,
			config: Arc::clone(&config),
			active: DenseSlotMap::with_key(),
			accepted: accepted_rx,
			online,
			active_info,
			online_when,
			dropped: JoinSet::new(),
			ticket_expiries: JoinSet::new(),
			catalog,
			metrics_labels,
		};

		tokio::spawn(worker.run());

		tracing::info!(
			stream_id = %config.stream_id.short(),
			network_id = %config.network_id.short(),
			"created new stream producer",
		);

		Handle {
			when,
			config,
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
					self.shutdown();
					break;
				}

				// Triggered when the publishing conditions for this producer
				// are met and it is considered online.
				() = &mut self.online_when => {
					self.on_online();
				}

				// Triggered when [`Acceptor`] accepts a new connection from a
				// remote consumer
				Some((link, criteria, peer)) = self.accepted.recv() => {
					self.accept(link, criteria, peer).await;
				}

				// Triggered when a new datum is produced for this stream
				// by the public api via the [`Producer`] handle.
				Some(datum) = self.data_rx.recv() => {
					self.fanout(datum);
				}

				// Triggered when any of the active remote consumer connections
				// is dropped for any reason.
				Some(Ok(sub_id)) = self.dropped.join_next() => {
					self.on_connection_dropped(sub_id);
				}

				// Triggered when a consumer's ticket expires.
				Some(Ok(sub_id)) = self.ticket_expiries.join_next() => {
					self.on_ticket_expired(sub_id);
				}

				// Re-evaluate require() predicates against active
				// consumers when the discovery catalog changes (e.g.
				// periodic peer announcements). This ensures RTT-based
				// requirements are enforced after paths settle.
				_ = self.catalog.changed() => {
					self.on_require_reeval();
				}
			}
		}
	}

	/// Forwards the given datum to all active remote consumers that match the
	/// criteria.
	///
	/// Notes:
	/// - The datum is serialized only once for all matching consumers.
	/// - If no consumers match the datum, it is sent to the undelivered sink if
	///   configured.
	/// - If a consumer is lagging behind and its channel is full, it may be
	///   disconnected based on the producer configuration.
	fn fanout(&self, item: D) {
		let mut bytes = OnceCell::<Result<Bytes, D::EncodeError>>::new();
		for (_, subscription) in &self.active {
			if subscription.criteria.matches(&item) {
				// Serialize the datum only once for all matching consumers,
				// if there is at least one consumer with criteria that matches.
				let bytes = match bytes.get_or_init(|| item.encode()) {
					Ok(bytes) => bytes,
					Err(e) => {
						tracing::error!(
							stream_id = %Short(self.config.stream_id),
							error = %e,
							"failed to serialize datum; dropping",
						);
						bytes.take();
						break;
					}
				};

				// forward the serialized datum to the matching consumer
				let labels = self.metrics_labels.as_slice();
				if subscription.bytes_tx.try_send(bytes.clone()).is_ok() {
					metrics::counter!("mosaik.streams.producer.items.sent", labels)
						.increment(1);
					metrics::counter!("mosaik.streams.producer.bytes.sent", labels)
						.increment(bytes.len() as u64);
				} else {
					metrics::counter!("mosaik.streams.producer.items.dropped", labels)
						.increment(1);
					// if the consumer is falling behind
					if self.config.disconnect_lagging {
						// and we are configured to disconnect lagging consumers,
						tracing::warn!(
							stream_id = %Short(self.config.stream_id),
							consumer_id = %Short(&subscription.peer.id()),
							lagging_by = self.config.buffer_size,
							"disconnecting lagging consumer",
						);

						// mark the subscription to be dropped due to lagging
						let _ = subscription.drop_requested.set(TooSlow.into());
					} else {
						tracing::trace!(
							stream_id = %Short(self.config.stream_id),
							consumer_id = %Short(&subscription.peer.id()),
							lagging_by = self.config.buffer_size,
							"dropping datum for lagging consumer",
						);
					}
				}
			}
		}

		// check if there were any matching consumers
		if bytes.get().is_none() {
			// no consumers matched, handle undelivered datum
			if let Some(undelivered) = &self.config.undelivered {
				// send the datum to the undelivered sink if configured
				let undelivered = undelivered
					.downcast_ref::<mpsc::UnboundedSender<D>>()
					.expect("datum type mismatch; this is a bug.");

				if undelivered.send(item).is_err() {
					tracing::warn!(
						stream_id = %Short(self.config.stream_id),
						"undelivered sink is closed; dropping datum",
					);
				}
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
		if self.active.len() >= self.config.max_consumers {
			tracing::warn!(
				consumer_id = %Short(&link.remote_id()),
				stream_id = %Short(self.config.stream_id),
				current_subscribers = %self.active.len(),
				"rejected consumer connection: no capacity",
			);

			let labels = self.metrics_labels.as_slice();
			metrics::counter!("mosaik.streams.producer.consumers.rejected", labels)
				.increment(1);

			// Close the link with `NoCapacity` reason, this producer has
			// reached its maximum number of allowed subscribers.
			let _ = link.close(NoCapacity).await;
			return;
		}

		// Sample RTT from the QUIC connection before evaluating require().
		// The connection is already established at this point, so the QUIC
		// stack has an RTT estimate from the handshake. Recording it now
		// ensures the require() predicate has RTT data available without
		// needing a separate ping probe.
		if let Some(rtt) = crate::discovery::rtt::best_rtt(link.connection()) {
			self.rtt.record_sample(*peer.id(), rtt);
		}

		// Check if we should accept this consumer based on the require predicate
		let peer_info = PeerInfo::from_tracker(&peer, &self.rtt);
		if !(self.config.require)(&peer_info) {
			tracing::warn!(
				stream_id = %Short(self.config.stream_id),
				consumer_id = %Short(&peer),
				"rejected consumer connection: unauthorized",
			);

			let labels = self.metrics_labels.as_slice();
			metrics::counter!("mosaik.streams.producer.consumers.rejected", labels)
				.increment(1);

			// Close the link with `NotAllowed` reason, this peer is not
			// authorized to subscribe to this stream.
			let _ = link.close(NotAllowed).await;
			return;
		}

		// Validate the consumer's ticket against all configured validators
		let Ok(ticket_expiration) =
			peer.validate_tickets(&self.config.ticket_validators)
		else {
			tracing::warn!(
				stream_id = %Short(self.config.stream_id),
				consumer_id = %Short(&peer),
				"rejected consumer connection: invalid ticket",
			);

			let labels = self.metrics_labels.as_slice();
			metrics::counter!("mosaik.streams.producer.consumers.rejected", labels)
				.increment(1);

			let _ = link.close(NotAllowed).await;
			return;
		};

		// create and spawn a new sender task for this consumer
		let (sub, info) = Sender::spawn(
			link,
			&self.config,
			&self.cancel,
			self.local_id,
			criteria,
			peer,
			&self.rtt,
		);

		// Add this consumer to the list of active subscriptions
		let sub_id = self.active.insert(sub);

		let labels = self.metrics_labels.as_slice();
		metrics::counter!("mosaik.streams.producer.consumers.accepted", labels)
			.increment(1);
		metrics::gauge!("mosaik.streams.producer.consumers", labels)
			.set(self.active.len() as f64);

		// Monitor the disconnection of this consumer
		let drop_fut = info.disconnected();
		self.dropped.spawn(drop_fut.map(move |()| sub_id));

		// Schedule ticket expiry timer if the ticket has an expiration
		if let Some(duration) = ticket_expiration.and_then(|e| e.remaining()) {
			self.ticket_expiries.spawn(async move {
				tokio::time::sleep(duration).await;
				sub_id
			});
		}

		// Update the active subscriptions info map and notify observers
		self.active_info.send_modify(|active| {
			let sub_id = Digest::from_u64(sub_id.0.as_ffi());
			active.insert(sub_id, info);
		});
	}

	/// Gracefully shuts down the worker loop by closing all active
	/// subscriptions.
	fn shutdown(&mut self) {
		tracing::debug!(
			stream_id = %Short(self.config.stream_id),
			"terminating stream producer",
		);

		self.cancel.cancel();
		self.dropped.abort_all();

		for (sub_id, subscription) in self.active.drain() {
			// Remove subscription from the active info map and signal
			// to status observers that the subscription is gone.
			self.active_info.send_modify(|active| {
				let sub_id = Digest::from_u64(sub_id.0.as_ffi());
				active.remove(&sub_id);
			});

			let _ = subscription.drop_requested.set(GracefulShutdown.into());
		}
	}

	/// Periodically re-evaluates `require()` predicates against active
	/// consumers with fresh `PeerInfo` (including current RTT). Consumers
	/// that no longer satisfy the predicate are disconnected, following
	/// the same pattern as ticket expiry.
	fn on_require_reeval(&self) {
		for (_, subscription) in &self.active {
			let info = PeerInfo::from_tracker(&subscription.peer, &self.rtt);
			if !(self.config.require)(&info) {
				tracing::debug!(
					stream_id = %Short(self.config.stream_id),
					consumer_id = %Short(&subscription.peer.id()),
					"consumer no longer satisfies require predicate; \
					 disconnecting",
				);
				let _ = subscription.drop_requested.set(NotAllowed.into());
			}
		}
	}

	/// Triggered when a consumer's ticket expires. Disconnects the consumer
	/// so it must re-authenticate on reconnection.
	fn on_ticket_expired(&self, sub_id: SubscriptionId) {
		if let Some(subscription) = self.active.get(sub_id) {
			tracing::debug!(
				stream_id = %Short(self.config.stream_id),
				consumer_id = %Short(&subscription.peer.id()),
				"consumer ticket expired; disconnecting",
			);

			let _ = subscription.drop_requested.set(NotAllowed.into());
		}
	}

	/// Triggered when we detect that a remote consumer connection has been
	/// dropped for any reason.
	fn on_connection_dropped(&mut self, sub_id: SubscriptionId) {
		// Remove subscription from the active info map and signal
		// to status observers that the subscription is gone.
		self.active_info.send_modify(|active| {
			let sub_id = Digest::from_u64(sub_id.0.as_ffi());
			active.remove(&sub_id);
		});

		if let Some(subscription) = self.active.remove(sub_id) {
			let labels = self.metrics_labels.as_slice();
			metrics::gauge!("mosaik.streams.producer.consumers", labels)
				.set(self.active.len() as f64);

			tracing::info!(
				stream_id = %Short(self.config.stream_id),
				consumer_id = %Short(&subscription.peer.id()),
				remaining_consumers = %self.active.len(),
				"consumer disconnected",
			);
		}

		if !self.online_when.is_condition_met() {
			tracing::trace!(
				stream_id = %Short(self.config.stream_id),
				consumers = %self.active.len(),
				"producer is offline",
			);
			self.online.send_replace(false);
		}
	}

	/// Triggered when the publishing conditions for this producer are met
	/// and it is considered online.
	fn on_online(&self) {
		tracing::trace!(
			stream_id = %Short(self.config.stream_id),
			consumers = %self.active.len(),
			"producer is online",
		);

		self.online.send_if_modified(|status| {
			if *status {
				false
			} else {
				*status = true;
				true
			}
		});
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
