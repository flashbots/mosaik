use {
	super::{
		super::{Datum, StreamId},
		Error,
		Status,
	},
	crate::{
		prelude::{Catalog, Criteria, DiscoveryEvent, Network, NetworkId, PeerId},
		streams::{
			link::{CloseReason, Link},
			protocol::SubscriptionRequest,
		},
	},
	bytes::BytesMut,
	core::{
		mem,
		pin::Pin,
		sync::atomic::Ordering,
		task::{Context, Poll},
		time::Duration,
	},
	dashmap::{DashMap, Entry},
	futures::{
		Stream,
		StreamExt,
		future::join_all,
		stream::{FuturesUnordered, StreamFuture},
	},
	iroh::{Endpoint, EndpointAddr},
	std::{io, sync::Arc, time::Instant},
	tokio::sync::mpsc::{self, error::TryRecvError},
	tokio_util::sync::{CancellationToken, DropGuard},
	tracing::{debug, info, warn},
};

/// A local stream consumer handle that allows receiving data from remote
/// producers.
///
/// Notes:
///
/// - Each consumer has an associated event loop task that manages finding and
///   connecting to producers for a given stream, as well as receiving data from
///   them.
///
/// - When this consumer is dropped, the event loop task is also terminated.
///
/// - Each consumer starts with no connected producers. It receives access to an
///   observable discovery catalog that allows it to find and connect to peers
///   producing the desired stream. As producers are discovered, the event loop
///   connects to them and starts receiving data.
pub struct Consumer<D: Datum> {
	stream_id: StreamId,
	status: Arc<Status>,
	data_rx: mpsc::Receiver<D>,
	_abort: DropGuard,
}

/// Public API
impl<D: Datum> Consumer<D> {
	/// Access to the status of this consumer.
	///
	/// The returned value can be used to query snapshots of statistics about
	/// the consumer, as well as to await important status changes.
	pub fn status(&self) -> &Status {
		&self.status
	}

	/// Returns the stream ID that this consumer is receiving data for.
	pub const fn stream_id(&self) -> &StreamId {
		&self.stream_id
	}

	/// Receives the next datum from this consumer.
	///
	/// If the consumer has been terminated, an error is returned.
	pub async fn recv(&mut self) -> Result<D, Error> {
		self.data_rx.recv().await.ok_or(Error::Terminated)
	}

	/// Polls for a pending datum from this consumer without waiting.
	pub fn try_recv(&mut self) -> Result<Option<D>, Error> {
		match self.data_rx.try_recv() {
			Ok(datum) => Ok(Some(datum)),
			Err(TryRecvError::Empty) => Ok(None),
			Err(TryRecvError::Disconnected) => Err(Error::Terminated),
		}
	}

	/// Receives the next datum from this consumer in a blocking manner.
	/// If the consumer has been terminated, an error is returned.
	pub fn blocking_recv(&mut self) -> Result<D, Error> {
		self.data_rx.blocking_recv().ok_or(Error::Terminated)
	}
}

/// Internal API
impl<D: Datum> Consumer<D> {
	pub fn new(network: &Network, criteria: Criteria) -> Self {
		let (event_loop, handle) = EventLoop::new(network, criteria);

		// Spawn the event loop task
		tokio::spawn(event_loop.run());

		handle
	}
}

impl<D: Datum> Stream for Consumer<D> {
	type Item = D;

	fn poll_next(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Self::Item>> {
		self.data_rx.poll_recv(cx)
	}
}

/// A long-running async task that manages finding and connecting to producers
/// for a given stream and receiving data from them.
///
/// Notes:
///
/// - This event loop is created together with a `Consumer` handle.
///
/// - When the `Consumer` handle is dropped, this event loop is also terminated.
struct EventLoop<D: Datum> {
	/// The network ID that this consumer is operating on.
	network_id: NetworkId,

	/// The stream ID that this consumer is receiving data for.
	stream_id: StreamId,

	/// The local endpoint that provides transport-level connectivity,
	/// used to connect to remote producers.
	endpoint: Endpoint,

	/// The discovery catalog used to query and observe changes to available
	/// remote producers.
	catalog: Catalog,

	/// The criteria used to filter which data is received from producers.
	criteria: Criteria,

	/// The status object used to track statistics about this consumer and signal
	/// important status changes.
	status: Arc<Status>,

	/// Channel used to send received datums to the consumer handle.
	data_tx: mpsc::Sender<D>,

	/// Active receiving streams from connected producers. This is a set of join
	/// handles for sub-loops that each manage receiving data from a single
	/// producer link.
	subs: FuturesUnordered<StreamFuture<Link>>,

	/// Map of peers that we are actively connected to or trying to connect to.
	active: DashMap<PeerId, ConnectionState>,

	/// Cancellation token used to terminate this event loop.
	cancel: CancellationToken,
}

impl<D: Datum> EventLoop<D> {
	pub fn new(network: &Network, criteria: Criteria) -> (Self, Consumer<D>) {
		let status = Arc::new(Status::new());
		let cancel = CancellationToken::new();
		let stream_id = StreamId::of::<D>();
		let (data_tx, data_rx) = mpsc::channel::<D>(1);

		let handle = Consumer {
			stream_id: stream_id.clone(),
			status: Arc::clone(&status),
			data_rx,
			_abort: cancel.clone().drop_guard(),
		};

		let event_loop = Self {
			network_id: network.network_id().clone(),
			stream_id,
			endpoint: network.local().endpoint().clone(),
			catalog: network.discovery().catalog().clone(),
			active: DashMap::new(),
			subs: FuturesUnordered::new(),
			status,
			criteria,
			data_tx,
			cancel,
		};

		(event_loop, handle)
	}
}

impl<D: Datum> EventLoop<D> {
	pub async fn run(mut self) {
		let mut discovery_events = self.catalog.watch();

		// start with subscribing to all known producers that are known in the
		// catalog at this time.
		self.subscribe_to_all().await;

		loop {
			tokio::select! {
				// Termination requested
				_ = self.cancel.cancelled() => {
					self.on_terminated().await;
					return;
				}

				// Handle discovery events
				Some(event) = discovery_events.next() => {
					self.on_discovery_event(event).await;
				}

				// Handle terminated receiver sub-loops
				Some((data, link)) = self.subs.next() => {
					self.on_data_received(data, link).await;
				}

				// Handle incoming data from producers
				// Some((data, link)) = self.recvs.next() => {
				// 	self.on_data_received(data, link).await;
				// }
			}
		}
	}

	/// Handles termination of the consumer by closing all active links
	/// to all connected producers.
	async fn on_terminated(&mut self) {
		for sub in mem::take(&mut self.subs).into_iter() {
			if let Some(link) = sub.into_inner() {
				self.disconnect(link, None).await;
			}
		}
	}

	/// Handles incoming data received from a remote producer link.
	async fn on_data_received(
		&mut self,
		data: Option<Result<BytesMut, io::Error>>,
		link: Link,
	) {
		let Some(result) = data else {
			warn!(
				stream_id = %self.stream_id,
				peer_id = %link.peer_id(),
				"Stream closed by producer");

			self.disconnect(link, Some(CloseReason::StreamError)).await;
			return;
		};

		let bytes = match result {
			Ok(bytes) => bytes,
			Err(e) => {
				// io error receiving data
				warn!(
					stream_id = %self.stream_id,
					peer_id = %link.peer_id(),
					error = %e,
					"Error receiving data");

				// Drop the link
				self.disconnect(link, Some(CloseReason::StreamError)).await;
				return;
			}
		};

		// Deserialize byte data into datum
		let datum: D = match rmp_serde::from_slice(&bytes[..]) {
			Ok(datum) => datum,
			Err(e) => {
				// drop any producer that sends invalid data
				warn!(
					error = %e,
					stream_id = %self.stream_id,
					peer_id = %link.peer_id(),
					"Received invalid data");

				self
					.disconnect(link, Some(CloseReason::ProtocolError))
					.await;

				return;
			}
		};

		// Send datum to consumer handle
		if self.data_tx.send(datum).await.is_err() {
			// consumer receiver dropped. Terminate the consumer loop, which
			// will close all active links with all remote producers.
			warn!(
					stream_id = %self.stream_id,
					peer_id = %link.peer_id(),
					"Consumer dropped; terminating consumer event loop");

			self.cancel.cancel();
			return;
		}

		// update stats
		self.status.items_received.fetch_add(1, Ordering::Relaxed);
		self
			.status
			.bytes_received
			.fetch_add(bytes.len() as u64, Ordering::Relaxed);

		// Re-insert the link to continue receiving data on it
		self.subs.push(link.into_future());
	}

	/// Attempts to subscribe to a remote producer for this stream.
	async fn subscribe(&self, addr: EndpointAddr) {
		let peer_id = addr.id;

		// ensure that we are not already connected to or in backoff state
		// or blocked. Immediately mark peers we can try to connect to as
		// "Connecting" to avoid duplicate connection attempts.
		let attempt_number = match self.active.entry(peer_id) {
			Entry::Occupied(mut entry) => match entry.get() {
				ConnectionState::Backoff { until, attempts }
					if Instant::now() >= *until =>
				{
					let attempts = attempts.saturating_add(1);
					entry.insert(ConnectionState::Connecting);
					attempts
				}
				_ => return,
			},
			Entry::Vacant(entry) => {
				entry.insert(ConnectionState::Connecting);
				0
			}
		}
		.saturating_add(1);

		// attempt to establish a connection
		let mut link = match Link::connect(self.endpoint.clone(), addr).await {
			Ok(link) => link,
			Err(e) => {
				warn!(
					stream_id = %self.stream_id,
					peer_id = %peer_id,
					error = %e,
					"Failed to connect to producer"
				);

				let backoff = ConnectionState::backoff(attempt_number);
				assert!(matches!(
					self.active.insert(peer_id, backoff),
					Some(ConnectionState::Connecting)
				));
				return;
			}
		};

		// we have a connection, send subscription request
		if let Err(e) = link
			.send_as(SubscriptionRequest {
				network_id: self.network_id.clone(),
				stream_id: self.stream_id.clone(),
				criteria: self.criteria.clone(),
			})
			.await
		{
			warn!(
				stream_id = %self.stream_id,
				peer_id = %peer_id,
				error = %e,
				"Failed to send subscription request"
			);

			let backoff = ConnectionState::backoff(attempt_number);
			assert!(matches!(
				self.active.insert(peer_id, backoff),
				Some(ConnectionState::Connecting)
			));
			return;
		}

		// Successfully connected and sent subscription request
		// Update the connection state for this peer.
		assert!(matches!(
			self.active.insert(peer_id, ConnectionState::Connected),
			Some(ConnectionState::Connecting)
		));

		// Start receiving data from this link
		self.subs.push(link.into_future());

		// update & notify status
		self.status.producers_count.fetch_add(1, Ordering::Relaxed);
		self.status.notify.notify_waiters();

		info!(
			peer_id = %peer_id,
			stream_id = %self.stream_id,
			criteria = ?self.criteria,
			"Subscribed to remote producer"
		);
	}

	// todo: consider adding backoff when there is a reason
	async fn disconnect(&self, link: Link, reason: Option<CloseReason>) {
		let peer_id = *link.peer_id();
		match self.active.remove(&peer_id) {
			Some((_, ConnectionState::Connected)) => {
				debug!(
					peer_id = %peer_id,
					stream_id = %self.stream_id,
					reason = ?reason,
					"Disconnecting from producer"
				);

				let close_result = match reason {
					Some(reason) => link.close_with_reason(reason).await,
					_ => link.close().await,
				};

				if let Err(e) = close_result {
					warn!(
						peer_id = %peer_id,
						stream_id = %self.stream_id,
						error = %e,
						"Error closing link");
				}

				self.status.producers_count.fetch_sub(1, Ordering::Relaxed);
				self.status.notify.notify_waiters();
			}
			_ => unreachable!("bug; disconnect called for non-connected peer"),
		}
	}

	/// Attempts to subscribe to all available producers for the stream that are
	/// currently known in the discovery catalog.
	async fn subscribe_to_all(&mut self) {
		// Try to subscribe to all known producers for this stream
		join_all(self.catalog.peers().filter_map(|peer| {
			peer
				.streams
				.contains(&self.stream_id)
				.then(|| self.subscribe(peer.address.clone()))
		}))
		.await;
	}

	async fn on_discovery_event(&mut self, event: DiscoveryEvent) {
		match event {
			DiscoveryEvent::New(peer_info) => {
				if peer_info.streams.contains(&self.stream_id) {
					self.subscribe(peer_info.address).await;
				}
			}
			DiscoveryEvent::Removed(_) => {
				// noop for now, if we're still connected to the producer,
				// wait for it to close the link.
			}
			DiscoveryEvent::Updated(peer_info) => {
				if peer_info.streams.contains(&self.stream_id) {
					self.subscribe(peer_info.address).await;
				}
			}
			DiscoveryEvent::SignificantlyChanged => {
				// do a full sweep and resubscribe to all unsubscribed producers
				self.subscribe_to_all().await
			}
		}
	}
}

enum ConnectionState {
	Connecting,
	Connected,
	Backoff { until: Instant, attempts: u32 },
}

impl ConnectionState {
	pub fn backoff(attempts: u32) -> Self {
		let until = Instant::now() + (attempts * Duration::from_secs(5));
		Self::Backoff { until, attempts }
	}
}

slotmap::new_key_type! {
	/// A unique identifier for a remote subscription for one stream.
	///
	/// Each subscription represents a connection to a remote producer
	/// from which we are receiving data for a given stream.
	///
	/// We may have multiple subscriptions for the same stream, to different producers.
	struct SubscriptionId;
}
