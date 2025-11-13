use {
	super::{
		super::{Datum, StreamId},
		Error,
		Status,
	},
	crate::{
		prelude::{Catalog, Criteria, DiscoveryEvent, Network, PeerId},
		streams::{
			link::{CloseReason, Link},
			protocol::SubscriptionRequest,
		},
	},
	bytes::BytesMut,
	core::{
		pin::Pin,
		sync::atomic::Ordering,
		task::{Context, Poll},
		time::Duration,
	},
	futures::{
		Stream,
		StreamExt,
		stream::{FuturesUnordered, StreamFuture},
	},
	iroh::Endpoint,
	std::{
		collections::{HashMap, hash_map::Entry},
		io,
		sync::Arc,
		time::Instant,
	},
	tokio::sync::mpsc::{self, error::TryRecvError},
	tokio_util::sync::{CancellationToken, DropGuard},
	tracing::warn,
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
	pub(crate) fn new(network: &Network, criteria: Criteria) -> Self {
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
	stream_id: StreamId,
	endpoint: Endpoint,
	catalog: Catalog,
	criteria: Criteria,
	status: Arc<Status>,
	data_tx: mpsc::Sender<D>,
	recvs: FuturesUnordered<StreamFuture<Link>>,
	peers: HashMap<PeerId, PeerStatus>,
	cancel: CancellationToken,
}

impl<D: Datum> EventLoop<D> {
	pub fn new(network: &Network, criteria: Criteria) -> (Self, Consumer<D>) {
		let status = Arc::new(Status::new());
		let cancel = CancellationToken::new();
		let (data_tx, data_rx) = mpsc::channel::<D>(1);

		let handle = Consumer {
			status: Arc::clone(&status),
			data_rx,
			_abort: cancel.clone().drop_guard(),
		};

		let event_loop = Self {
			stream_id: StreamId::of::<D>(),
			endpoint: network.local().endpoint().clone(),
			catalog: network.discovery().catalog().clone(),
			peers: HashMap::new(),
			recvs: FuturesUnordered::new(),
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

				// Handle incoming data from producers
				Some((data, link)) = self.recvs.next() => {
					self.on_data_received(data, link).await;
				}
			}
		}
	}

	/// Handles termination of the consumer by closing all active links
	/// to all connected producers.
	async fn on_terminated(&mut self) {
		let subs = core::mem::take(&mut self.recvs)
			.into_iter()
			.collect::<Vec<_>>();

		for sub in subs {
			if let Some(link) = sub.into_inner() {
				self.disconnect(link).await;
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
			// Link closed, drop the producer
			self.disconnect(link).await;
			return;
		};

		match result {
			Ok(bytes) => {
				match rmp_serde::from_slice(&bytes[..]) {
					Ok(datum) => {
						// Send datum to consumer
						if let Err(e) = self.data_tx.send(datum).await {
							// consumer receiver dropped. Terminate the consumer loop.
							warn!("Failed to send datum to consumer: {e}");
							self.cancel.cancel();
							return;
						}

						// Re-insert the link to continue receiving data
						self.recvs.push(link.into_future());
					}
					Err(e) => {
						warn!(
              stream_id = %self.stream_id,
              peer_id = %link.peer_id(),
              "Failed to deserialize datum: {e}");

						// Drop the link
						self.disconnect(link).await;
					}
				}
			}
			Err(e) => {
				warn!(
          stream_id = %self.stream_id,
          peer_id = %link.peer_id(),
          "Error receiving data from link: {e}");

				// Drop the link
				self.disconnect(link).await;
			}
		}
	}

	async fn disconnect(&mut self, link: Link) {
		let peer_id = *link.peer_id();
		if let Some(PeerStatus::Connected) = self.peers.remove(&peer_id) {
			if let Err(e) = link.close_with_reason(CloseReason::Unspecified).await {
				warn!(
        stream_id = %self.stream_id,
        peer_id = %peer_id,
        "Error closing link: {e}");
			}

			self.status.producers_count.fetch_sub(1, Ordering::Relaxed);
			self.status.notify.notify_waiters();
		}
	}

	/// Attempts to subscribe to all available producers for the stream that are
	/// currently known in the discovery catalog.
	async fn subscribe_to_all(&mut self) {
		let mut tasks = FuturesUnordered::new();

		for peer in self.catalog.peers() {
			if !peer.streams.contains(&self.stream_id) {
				continue;
			}

			match self.peers.entry(peer.address.id) {
				Entry::Occupied(mut occupied) => {
					match occupied.get() {
						PeerStatus::Connecting => continue,
						PeerStatus::Connected => continue,
						PeerStatus::BackoffUntil(instant) => {
							if Instant::now() < *instant {
								continue;
							}
							occupied.insert(PeerStatus::Connecting);
							todo!()
						}
						PeerStatus::Blocked => continue,
					};
				}
				Entry::Vacant(vacant) => {
					vacant.insert(PeerStatus::Connecting);
					let endpoint = self.endpoint.clone();
					let addr = peer.address.clone();

					// begin connecting to the producer
					tasks.push(
						async move { (addr.id, Link::connect(endpoint, addr).await) },
					);
				}
			};
		}

		while let Some((peer_id, result)) = tasks.next().await {
			match result {
				Ok(mut link) => {
					// send handshake / subscription request
					if let Err(e) = link
						.send_as(SubscriptionRequest {
							stream_id: self.stream_id.clone(),
							criteria: self.criteria.clone(),
						})
						.await
					{
						warn!(
							stream_id = %self.stream_id,
							peer_id = %peer_id,
							"Failed to send subscription request: {e}"
						);

						let backoff = Instant::now() + Duration::from_secs(5);
						self
							.peers
							.insert(peer_id, PeerStatus::BackoffUntil(backoff));
						continue;
					}

					self.recvs.push(link.into_future());
					let prev_state = self.peers.insert(peer_id, PeerStatus::Connected);
					assert!(matches!(prev_state, Some(PeerStatus::Connecting)));
					self.status.producers_count.fetch_add(1, Ordering::Relaxed);
					self.status.notify.notify_waiters();
				}
				Err(e) => {
					warn!(
						stream_id = %self.stream_id,
						peer_id = %peer_id,
						"Failed to connect to producer: {e}"
					);

					let backoff = Instant::now() + Duration::from_secs(5);
					self
						.peers
						.insert(peer_id, PeerStatus::BackoffUntil(backoff));
				}
			};
		}
	}

	async fn on_discovery_event(&mut self, event: DiscoveryEvent) {
		match event {
			DiscoveryEvent::New(peer_info) => {
				if peer_info.streams.contains(&self.stream_id)
					&& !self.peers.contains_key(&peer_info.address.id)
				{
					// New producer for us
					self.subscribe_to_all().await;
				}
			}
			DiscoveryEvent::Removed(_) => {
				// Handle removed peer
			}
			DiscoveryEvent::Updated(peer_info) => {
				if peer_info.streams.contains(&self.stream_id)
					&& !self.peers.contains_key(&peer_info.address.id)
				{
					// New producer for us
					self.subscribe_to_all().await;
				}
			}
			DiscoveryEvent::SignificantlyChanged => {
				// do a full sweep and resubscribe to all unsubscribed producers
				self.subscribe_to_all().await
			}
		}
	}
}

enum PeerStatus {
	Connecting,
	Connected,
	BackoffUntil(Instant),
	Blocked,
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
