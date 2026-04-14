use {
	crate::{
		Datum,
		PeerId,
		discovery::{Catalog, Discovery, rtt::PeerInfo},
		network::LocalNode,
		primitives::{Digest, Short},
		streams::{
			Consumer,
			Streams,
			consumer::{builder::ConsumerConfig, receiver::Receiver},
			status::{ActiveChannelsMap, ChannelConditions, State, Stats, When},
		},
	},
	core::pin::Pin,
	futures::{Stream, StreamExt, stream::SelectAll},
	std::{collections::HashMap, sync::Arc},
	tokio::{
		sync::{mpsc, watch},
		task::JoinSet,
	},
	tokio_stream::wrappers::WatchStream,
	tokio_util::sync::CancellationToken,
};

/// Worker task that manages the state of one consumer for a specific datum type
/// `D` on the local node.
///
/// Notes:
/// - Individual worker tasks are spawned for each subscribed producer peer.
pub(super) struct ConsumerWorker<D: Datum> {
	/// The consumer-specific configuration as assembled by
	/// `Network::streams().consumer()`. If some configuration values are not
	/// set, they default to the values from `Streams` config.
	config: Arc<ConsumerConfig>,

	/// A handle to the local node that is used to establish new connections.
	local: LocalNode,

	/// The discovery system handle used to monitor known peers that are
	/// producing the desired stream and also to trigger catalog syncs when
	/// the consumer is connecting to a producer that does not recognize it.
	discovery: Discovery,

	/// Channel for sending received data to the consumer handle.
	data_tx: mpsc::UnboundedSender<(D, usize)>,

	/// Triggered when the consumer handle is dropped.
	cancel: CancellationToken,

	/// Active receive workers for connected producer peers.
	/// This value can be observed to get the current snapshot of connected
	/// producers.
	active: watch::Sender<ActiveChannelsMap>,

	/// Aggregated status streams from all active receiver workers.
	status_rx: StateUpdatesStream,

	/// Cancellation tokens for individual receiver workers, keyed by the same
	/// `sub_id` used in the `active` map. Cancelling a token terminates its
	/// corresponding receiver.
	receiver_cancels: HashMap<Digest, CancellationToken>,

	/// Sets the online status of the consumer when the configured online
	/// conditions are met.
	online: watch::Sender<bool>,

	/// A future that resolves when the consumer meets the configured online
	/// conditions.
	online_when: ChannelConditions,

	/// Futures that resolve when a producer's ticket expires.
	ticket_expiries: JoinSet<Digest>,

	/// Pending RTT probes for newly discovered producers that have no
	/// RTT data yet. When a probe completes, the result is fed back to
	/// the discovery system, which triggers a catalog update that
	/// re-evaluates `require()` with RTT data available.
	rtt_probes: JoinSet<()>,
}

impl<D: Datum> ConsumerWorker<D> {
	/// Spawns a new receive worker task and returns the consumer handle that can
	/// receive data and query status.
	///
	/// The worker handle will terminate when the returned consumer is dropped.
	pub fn spawn(config: ConsumerConfig, streams: &Streams) -> Consumer<D> {
		let config = Arc::new(config);
		let local = streams.local.clone();
		let cancel = local.termination().child_token();
		let active = watch::Sender::new(ActiveChannelsMap::new());
		let online = watch::Sender::new(false);
		let (data_tx, data_rx) = mpsc::unbounded_channel();

		let when = When::new(active.subscribe(), online.subscribe());
		let online_when = (config.online_when)(when.subscribed());

		online.send_replace(online_when.is_condition_met());

		let worker = Self {
			local,
			data_tx,
			config: Arc::clone(&config),
			discovery: streams.discovery.clone(),
			cancel: cancel.clone(),
			active: active.clone(),
			status_rx: StateUpdatesStream::new(),
			receiver_cancels: HashMap::new(),
			online: online.clone(),
			online_when,
			ticket_expiries: JoinSet::new(),
			rtt_probes: JoinSet::new(),
		};

		tokio::spawn(worker.run());

		Consumer {
			config: Arc::clone(&config),
			chan: data_rx,
			stats: Stats::default_connected(),
			status: When::new(active.subscribe(), online.subscribe()),
			_abort: cancel.drop_guard(),
		}
	}
}

impl<D: Datum> ConsumerWorker<D> {
	async fn run(mut self) {
		// Get a watch handle for the discovery catalog
		// and mark it as changed to trigger an initial producers lookup in the
		// current state of the catalog.
		let mut catalog = self.discovery.catalog_watch();
		catalog.mark_changed();

		loop {
			tokio::select! {
				// Triggered when the consumer is dropped or the network is shutting down
				() = self.cancel.cancelled() => {
					self.on_terminated();
					break;
				}

				// Triggered when the online conditions for this consumer
				// are met.
				() = &mut self.online_when => {
					self.on_online();
				}

				// Triggered when new peers are discovered or existing peers are updated
				_ = catalog.changed() => {
					// mark the latest catalog snapshot as seen and trigger peers scan
					let snapshot = catalog.borrow_and_update().clone();
					self.on_catalog_update(snapshot);
				}

				// Triggered when one of the active receiver workers updates its state
				Some((state, peer_id)) = self.status_rx.next() => {
					self.on_receiver_state_update(peer_id, state);
				}

				// Triggered when a producer's ticket expires.
				Some(Ok(sub_id)) = self.ticket_expiries.join_next() => {
					self.on_ticket_expired(sub_id);
				}

				// Drive RTT probe tasks (results are fed back to
				// discovery, triggering a catalog update that
				// re-evaluates require() with RTT data).
				Some(_) = self.rtt_probes.join_next() => {}
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
		// identify all producers that are producing the desired stream id
		// and satisfy the user-provided additional eligibility criteria.
		let producers = latest
			.peers()
			.filter(|peer| peer.streams().contains(&self.config.stream_id));

		for producer in producers {
			// for each discovered producer, create a receive worker if it is not
			// already in the active list of connected producers.
			let sub_id = Digest::from_bytes(*producer.id().as_bytes());
			if !self.active.borrow().contains_key(&sub_id) {
				tracing::trace!(
					stream_id = %Short(self.config.stream_id),
					producer = %Short(producer),
					network = %producer.network_id(),
					"discovered new stream producer"
				);

				// If no RTT data exists for this producer, trigger an RTT ping probe
				// before evaluating require(). The probe feeds its result back to the
				// discovery system, which triggers a catalog update that re-evaluates
				// with RTT data available. This avoids wasteful connections to
				// peers that will fail RTT requirements.
				if self.discovery.rtt_tracker().get(producer.id()).is_none() {
					let local = self.local.clone();
					let discovery = self.discovery.clone();
					let addr = producer.address().clone();
					self.rtt_probes.spawn(async move {
						if let Ok((entry, rtt)) = local.ping(addr, None).await
							&& let Some(rtt) = rtt
						{
							discovery.rtt_tracker().record_sample(*entry.id(), rtt);
							discovery.feed(entry);
						}
					});
					continue;
				}

				let info =
					PeerInfo::from_tracker(producer, self.discovery.rtt_tracker());
				if !(self.config.require)(&info) {
					tracing::debug!(
						stream_id = %Short(self.config.stream_id),
						producer_id = %Short(producer),
						network = %producer.network_id(),
						"skipping ineligible producer"
					);
					continue;
				}

				// Validate the producer's ticket against all validators
				let Ok(ticket_expiration) =
					producer.validate_tickets(&self.config.ticket_validators)
				else {
					tracing::debug!(
						stream_id = %Short(self.config.stream_id),
						producer_id = %Short(producer),
						network = %producer.network_id(),
						"skipping unauthorized producer"
					);
					continue;
				};

				// create a per-receiver cancellation token so the consumer worker
				// can terminate this specific receiver independently
				let receiver_cancel = self.cancel.child_token();

				// spawn a new receiver worker for this producer and track its status
				let channel_info = Receiver::spawn(
					producer.clone(),
					&self.local,
					&self.discovery,
					&receiver_cancel,
					&self.data_tx,
					Arc::clone(&self.config),
				);

				// subscribe to the receiver's status updates
				let peer_id = *producer.id();
				self.status_rx.push(
					WatchStream::new(channel_info.state.clone())
						.map(move |state| (state, peer_id))
						.boxed(),
				);

				// track the active receiver handle, when workers terminate they will
				// transition into the terminated state and be removed from the active
				// list
				self.active.send_modify(|active| {
					active.insert(sub_id, channel_info);
				});
				self.receiver_cancels.insert(sub_id, receiver_cancel);

				// Schedule ticket expiry timer if the ticket has an expiration
				if let Some(duration) = ticket_expiration.and_then(|e| e.remaining()) {
					self.ticket_expiries.spawn(async move {
						tokio::time::sleep(duration).await;
						sub_id
					});
				}
			}
		}

		self.disconnect_ineligible_producers(&latest);
	}

	/// Re-evaluates the `require` predicate and ticket validity for all
	/// active connections. Disconnects receivers whose producers no
	/// longer satisfy the eligibility criteria or left the catalog.
	fn disconnect_ineligible_producers(&mut self, latest: &Catalog) {
		let to_disconnect: Vec<(Digest, PeerId)> = self
			.active
			.borrow()
			.iter()
			.filter_map(|(sub_id, info)| {
				let peer_id = *info.producer_id();
				let dominated = latest.get(&peer_id).is_none_or(|entry| {
					let info =
						PeerInfo::from_tracker(entry, self.discovery.rtt_tracker());
					!entry.streams().contains(&self.config.stream_id)
						|| !(self.config.require)(&info)
						|| entry
							.validate_tickets(&self.config.ticket_validators)
							.is_err()
				});
				dominated.then_some((*sub_id, peer_id))
			})
			.collect();

		for (sub_id, peer_id) in &to_disconnect {
			tracing::info!(
				producer_id = %Short(peer_id),
				stream_id = %Short(self.config.stream_id),
				"disconnecting ineligible producer"
			);

			if let Some(cancel) = self.receiver_cancels.remove(sub_id) {
				cancel.cancel();
			}
			self
				.active
				.send_if_modified(|active| active.remove(sub_id).is_some());
		}

		if !to_disconnect.is_empty() && !self.online_when.is_condition_met() {
			tracing::trace!(
				stream_id = %Short(self.config.stream_id),
				producers = %self.active.borrow().len(),
				"consumer is offline",
			);
			self.online.send_replace(false);
		}
	}

	/// Triggered when a producer's ticket expires. Disconnects the receiver
	/// so that a fresh ticket is validated on the next catalog update.
	fn on_ticket_expired(&mut self, sub_id: Digest) {
		if !self.active.borrow().contains_key(&sub_id) {
			return;
		}

		tracing::debug!(
			stream_id = %Short(self.config.stream_id),
			"producer ticket expired; disconnecting",
		);

		if let Some(cancel) = self.receiver_cancels.remove(&sub_id) {
			cancel.cancel();
		}
		self
			.active
			.send_if_modified(|active| active.remove(&sub_id).is_some());

		if !self.online_when.is_condition_met() {
			self.online.send_replace(false);
		}
	}

	/// Handles state updates from remote receiver workers.
	fn on_receiver_state_update(&mut self, peer_id: PeerId, state: State) {
		if state == State::Terminated {
			// The receiver has unrecoverably terminated, remove it from the active
			// list and clean up its cancellation token
			let sub_id = Digest::from_bytes(*peer_id.as_bytes());

			self
				.active
				.send_if_modified(|active| active.remove(&sub_id).is_some());
			self.receiver_cancels.remove(&sub_id);

			tracing::info!(
				producer_id = %Short(&peer_id),
				stream_id = %Short(self.config.stream_id),
				criteria = ?self.config.criteria,
				"connection with producer terminated"
			);

			if !self.online_when.is_condition_met() {
				tracing::trace!(
					stream_id = %Short(self.config.stream_id),
					producers = %self.active.borrow().len(),
					"consumer is offline",
				);
				self.online.send_replace(false);
			}
		}
	}

	/// Triggered when the online conditions for this consumer are met.
	fn on_online(&self) {
		tracing::trace!(
			stream_id = %Short(self.config.stream_id),
			producers = %self.active.borrow().len(),
			"consumer is online",
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

	/// Gracefully closes all connections with remote producers.
	fn on_terminated(&mut self) {
		// terminate all active receiver workers
		let producers_count = self.active.borrow().len();
		self.active.send_replace(ActiveChannelsMap::default());
		self.receiver_cancels.clear();

		tracing::debug!(
			stream_id = %Short(self.config.stream_id),
			producers_count = producers_count,
			criteria = ?self.config.criteria,
			"consumer terminated"
		);
	}
}

type StateUpdatesStream =
	SelectAll<Pin<Box<dyn Stream<Item = (State, PeerId)> + Send>>>;
