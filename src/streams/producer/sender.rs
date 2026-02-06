use {
	super::{
		super::{
			Criteria,
			Streams,
			accept::StartStream,
			status::{ChannelInfo, State, Stats},
		},
		builder::ProducerConfig,
	},
	crate::{
		PeerId,
		discovery::PeerEntry,
		network::{
			Cancelled,
			GracefulShutdown,
			UnexpectedClose,
			link::{Link, LinkError},
		},
		primitives::Short,
	},
	bytes::Bytes,
	iroh::endpoint::{ApplicationClose, ConnectionError},
	std::sync::Arc,
	tokio::sync::{SetOnce, mpsc, watch},
	tokio_util::sync::CancellationToken,
};

/// Represents an active subscription handle from a remote consumer.
/// Each subscription is an independent transport-level connection.
///
/// This type is used to control and interact with the sender task
/// responsible for sending datum to one connected consumer.
pub(super) struct Subscription {
	/// The `PeerEntry` of the remote consumer at the time of connection.
	pub peer: Arc<PeerEntry>,

	/// The stream subscription criteria for this consumer.
	pub criteria: Criteria,

	/// Channel for sending serialized datum buffers to the remote consumer.
	pub bytes_tx: mpsc::Sender<Bytes>,

	/// Signals when this subscription has been requested to be dropped.
	pub drop_requested: Arc<SetOnce<ApplicationClose>>,
}

/// Worker task that manages sending datum to one connected remote consumer.
///
/// Notes:
///
/// - Different consumers might be receiving datum at different rates. Each
///   sender task operates independently to avoid blocking the entire producer
///   when one consumer is slow or unresponsive.
///
/// - Mosaik guarantees in-order delivery of datum to each connected consumer.
///   The sender task ensures that datum are sent in the order they were
///   produced.
pub(super) struct Sender {
	/// The producer configuration associated with this stream id.
	config: Arc<ProducerConfig>,

	/// A snapshot of the peer entry for the remote consumer at the time of
	/// connection establishment.
	peer: Arc<PeerEntry>,

	/// Holds serialized datums pending to be sent to the remote consumer.
	bytes_rx: mpsc::Receiver<Bytes>,

	/// The physical transport link to the remote consumer over which datum
	/// are sent.
	link: Link<Streams>,

	/// Statistics tracker for this consumer.
	stats: Arc<Stats>,

	/// State watcher for this subscription.
	state: watch::Sender<State>,

	/// Cancellation token to signal shutdown of this sender task.
	/// This token is inherited from the producer to allow coordinated
	/// shutdown of all sender tasks when the producer is dropped. But it can
	/// also be used to shutdown this specific sender on unrecoverable errors.
	cancel: CancellationToken,

	/// Channel for receiving drop signals for this subscription.
	drop: Arc<SetOnce<ApplicationClose>>,
}

impl Sender {
	/// Spawns a new sender task for a connected consumer.
	///
	/// Returns the status tracker and a channel for sending serialized
	/// buffers to the sender task. Datums sent over the returned channel must
	/// meet the subscription criteria of the connected consumer. It is the role
	/// of the stream producer worker to enforce that.
	pub fn spawn(
		link: Link<Streams>,
		config: &Arc<ProducerConfig>,
		cancel: &CancellationToken,
		local_id: PeerId,
		criteria: Criteria,
		peer: PeerEntry,
	) -> (Subscription, ChannelInfo) {
		assert_eq!(peer.id(), &link.remote_id());

		let mut link = link;
		let cancel = cancel.child_token();
		link.replace_cancel_token(cancel.clone());

		let peer = Arc::new(peer);

		let drop = Arc::new(SetOnce::new());
		let (state_tx, state_rx) = watch::channel(State::Connecting);
		let (bytes_tx, bytes_rx) = mpsc::channel::<Bytes>(config.buffer_size);

		let stats = Arc::new(Stats::default());

		let status = ChannelInfo {
			criteria: criteria.clone(),
			stats: Arc::clone(&stats),
			peer: Arc::clone(&peer),
			stream_id: config.stream_id,
			producer_id: local_id,
			consumer_id: link.remote_id(),
			state: state_rx,
		};

		let sub = Subscription {
			criteria,
			bytes_tx,
			peer: Arc::clone(&peer),
			drop_requested: Arc::clone(&drop),
		};

		let worker = Self {
			link,
			peer,
			bytes_rx,
			cancel,
			drop,
			config: Arc::clone(config),
			stats: Arc::clone(&stats),
			state: state_tx,
		};

		tokio::spawn(worker.run());

		(sub, status)
	}
}

impl Sender {
	async fn run(mut self) {
		// Confirm the subscription with the remote consumer
		self.confirm_subscription().await;
		let mut disconnected = core::pin::pin!(self.link.closed());

		loop {
			tokio::select! {
				// Triggered when the producer or the network is shutting down
				() = self.cancel.cancelled() => {
					self.terminate(Cancelled).await;
					return;
				}

				// Triggered when the remote consumer link is dropped
				disconnected = &mut disconnected => {
					self.on_remote_link_dropped(disconnected);
				}

				// Triggered when there is a datum that matches the
				// subscription criteria of this connected consumer.
				Some(item) = self.bytes_rx.recv() => {
					self.send_item(item).await;
				}

				// Triggered when the producer has requested this
				// subscription to be dropped.
				reason = self.drop.wait() => {
					let reason = reason.clone();
					self.terminate(reason).await;
					return;
				}
			}
		}
	}

	/// Triggered when there is a datum to be sent to the remote consumer.
	/// Responsible for delivering the datum over the transport link.
	async fn send_item(&mut self, datum: Bytes) {
		// SAFETY: `datum` is already serialized by the producer worker
		let send_fut = unsafe { self.link.send_raw(datum) };

		match send_fut.await {
			Ok(bytes_len) => {
				self.stats.increment_datums();
				self.stats.increment_bytes(bytes_len);
			}
			Err(error) if !error.is_cancelled() => {
				tracing::debug!(
					error = %error,
					stream_id = %Short(self.config.stream_id),
					consumer_id = %Short(*self.peer.id()),
					"error while sending datum to consumer; disconnecting",
				);

				self.request_disconnect(error);
			}
			_ => { /* ignore cancelled errors */ }
		}
	}

	/// Called at the beginning of the sender loop to confirm
	/// the subscription with the remote consumer.
	async fn confirm_subscription(&mut self) {
		let config = &self.config;
		let start_message = StartStream(config.network_id, config.stream_id);

		match self.link.send(&start_message).await {
			Ok(_) => {
				tracing::trace!(
					stream_id = %Short(config.stream_id),
					consumer_id = %Short(*self.peer.id()),
					"confirmed subscription with consumer",
				);

				self.stats.connected();
				self.state.send_replace(State::Connected);
			}
			Err(error) => {
				tracing::warn!(
					error = %error,
					stream_id = %Short(config.stream_id),
					consumer_id = %Short(*self.peer.id()),
					"failed to confirm subscription with consumer; disconnecting",
				);
			}
		}
	}

	/// Handles the remote consumer link being dropped.
	fn on_remote_link_dropped(&self, result: Result<(), ConnectionError>) {
		let reason = match result {
			Ok(()) => GracefulShutdown.into(),
			Err(ConnectionError::ApplicationClosed(reason)) => reason,
			Err(error) => {
				tracing::debug!(
					error = %error,
					stream_id = %Short(self.config.stream_id),
					consumer_id = %Short(*self.peer.id()),
					"remote consumer link dropped unexpectedly",
				);
				UnexpectedClose.into()
			}
		};

		self.drop.set(reason).ok();
	}

	/// Terminates this sender task and closes the link to the remote consumer.
	async fn terminate(self, reason: impl Into<ApplicationClose>) {
		let reason = reason.into();
		self.state.send_replace(State::Terminated);
		self.stats.disconnected();

		if let Err(error) = self.link.close(reason.clone()).await {
			if !error.is_cancelled() && !error.was_already_closed() {
				tracing::debug!(
					error = %error,
					stream_id = %Short(self.config.stream_id),
					consumer_id = %Short(*self.peer.id()),
					"error while disconnecting consumer",
				);
			}
		}

		tracing::trace!(
			reason = ?reason,
			stream_id = %Short(self.config.stream_id),
			consumer_id = %Short(*self.peer.id()),
			"consumer subscription terminated",
		);
	}

	/// Called when we want to initiate the termination of this consumer
	/// connection.
	fn request_disconnect(&self, error: impl Into<LinkError>) {
		let error = error.into();
		let reason = error
			.close_reason()
			.cloned()
			.unwrap_or_else(|| UnexpectedClose.into());
		self.drop.set(reason).ok();
	}
}
