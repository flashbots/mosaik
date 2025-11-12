use {
	crate::{
		discovery::Catalog,
		prelude::DiscoveryEvent,
		streams::{
			Criteria,
			Datum,
			Error,
			StreamId,
			Streams,
			protocol::{SubscriptionRequest, SubscriptionResponse},
		},
	},
	bytes::BytesMut,
	core::{
		pin::Pin,
		sync::atomic::{AtomicU32, Ordering},
		task::{Context, Poll},
	},
	futures::{SinkExt, Stream, StreamExt, stream::SelectAll},
	iroh::{
		Endpoint,
		EndpointAddr,
		endpoint::{Connection, RecvStream, SendStream},
	},
	std::{io, sync::Arc},
	tokio::{
		io::Join,
		sync::{Notify, mpsc},
	},
	tokio_util::{
		codec::{Framed, LengthDelimitedCodec},
		sync::{CancellationToken, DropGuard},
	},
	tracing::{debug, info, warn},
};

pub struct Consumer<D: Datum> {
	stream_id: StreamId,
	criteria: Criteria,
	ready: Arc<Notify>,
	sources_count: Arc<AtomicU32>,
	item_rx: mpsc::Receiver<D>,
	_abort: DropGuard,
}

impl<D: Datum> Consumer<D> {
	pub fn new(
		endpoint: Endpoint,
		discovery: Catalog,
		criteria: Criteria,
	) -> Self {
		let stream_id = StreamId::of::<D>();
		let mut runloop = EventLoop::<D>::new(
			endpoint,
			discovery,
			stream_id.clone(),
			criteria.clone(),
		);

		let abort = runloop.drop_guard();
		let ready = Arc::clone(&runloop.ready);
		let sources_count = Arc::clone(&runloop.sources_count);
		let Some(item_rx) = runloop.take_data_rx() else {
			unreachable!("receiver should not be taken; qed");
		};

		tokio::spawn(runloop.run());

		Self {
			stream_id,
			criteria,
			item_rx,
			ready,
			sources_count,
			_abort: abort,
		}
	}

	pub const fn stream_id(&self) -> &StreamId {
		&self.stream_id
	}

	pub const fn criteria(&self) -> &Criteria {
		&self.criteria
	}

	pub fn sources_count(&self) -> u32 {
		self.sources_count.load(Ordering::Relaxed)
	}

	/// Awaits until the consumer is ready to receive data and is connected to at
	/// least one producer.
	pub async fn online(&self) {
		if self.sources_count() == 0 {
			self.ready.notified().await;
		}
	}
}

impl<D: Datum> Stream for Consumer<D> {
	type Item = D;

	fn poll_next(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Self::Item>> {
		self.get_mut().item_rx.poll_recv(cx)
	}
}

struct EventLoop<D: Datum> {
	local: Endpoint,
	catalog: Catalog,
	stream_id: StreamId,
	criteria: Criteria,
	ready: Arc<Notify>,
	sources_count: Arc<AtomicU32>,
	data_tx: mpsc::Sender<D>,
	data_rx: Option<mpsc::Receiver<D>>,
	cancel: CancellationToken,
	subscriptions: SelectAll<Subscription>,
	_marker: std::marker::PhantomData<D>,
}

impl<D: Datum> EventLoop<D> {
	pub fn new(
		local: Endpoint,
		peers: Catalog,
		stream_id: StreamId,
		criteria: Criteria,
	) -> Self {
		let cancel = CancellationToken::new();
		let (data_tx, data_rx) = mpsc::channel(1);
		let subscriptions = SelectAll::new();

		Self {
			local,
			stream_id,
			criteria,
			cancel,
			catalog: peers,
			ready: Arc::new(Notify::new()),
			sources_count: Arc::new(AtomicU32::new(0)),
			data_tx,
			data_rx: Some(data_rx),
			subscriptions,
			_marker: std::marker::PhantomData,
		}
	}

	pub fn drop_guard(&self) -> DropGuard {
		self.cancel.clone().drop_guard()
	}

	pub fn take_data_rx(&mut self) -> Option<mpsc::Receiver<D>> {
		self.data_rx.take()
	}

	pub async fn run(mut self) {
		let mut discovery_events = self.catalog.watch();

		// create subscriptions with known peers that have this stream
		for peer in self.catalog.peers() {
			if peer.streams.contains(&self.stream_id) {
				info!(
					stream = %self.stream_id,
					peer = %peer.id(),
					"Found existing peer with stream; creating subscription",
				);
				let peer_id = peer.id();
				let subscription = self.connect(peer.address).await;

				info!(
					stream = %self.stream_id,
					peer = %peer_id,
					"Subscription established",
				);

				self.subscriptions.push(subscription.unwrap());

				if self.subscriptions.len() == 1 {
					self.ready.notify_waiters();
				}
			}
		}

		loop {
			tokio::select! {
				_ = self.cancel.cancelled() => {
					info!(stream = %self.stream_id, "Consumer event loop terminated");
					break;
				}
				Some(item) = self.subscriptions.next() => {
					// process incoming data
					self.on_data_received(item).await;
				}

				Some(event) = discovery_events.next() => {
					self.on_discovery_event(event).await;
				}
			}
		}
	}

	async fn on_discovery_event(&mut self, event: DiscoveryEvent) {
		match event {
			DiscoveryEvent::New(peer) => {
				info!(stream = %self.stream_id, "New peer {peer} discovered");
				if peer.streams.contains(&self.stream_id) {
					info!(
						stream = %self.stream_id,
						peer = %peer.id(),
						"Found existing peer with stream; creating subscription",
					);
					let peer_id = peer.id();
					let subscription = self.connect(peer.address).await;

					info!(
						stream = %self.stream_id,
						peer = %peer_id,
						"Subscription established",
					);

					self.subscriptions.push(subscription.unwrap());

					if self.subscriptions.len() == 1 {
						self.ready.notify_waiters();
					}
				}
			}
			DiscoveryEvent::Updated(peer) => {
				info!(stream = %self.stream_id, "Peer {peer} updated in catalog");
			}
			DiscoveryEvent::Removed(peer) => {
				info!(stream = %self.stream_id, "Peer {peer} removed from catalog");
			}
			DiscoveryEvent::SignificantlyChanged => {
				info!(stream = %self.stream_id, "Catalog significantly changed");
			}
		}
	}

	async fn on_data_received(&mut self, packet: Result<BytesMut, io::Error>) {
		match packet {
			Ok(bytes) => {
				let data: D = rmp_serde::from_slice(&bytes)
					.map_err(|e| {
						io::Error::new(
							io::ErrorKind::InvalidData,
							format!("Failed to deserialize datum: {e}"),
						)
					})
					.unwrap();

				info!("Received datum from subscription: {data:?}");

				if let Err(e) = self.data_tx.send(data).await {
					warn!("Failed to send datum to consumer: {e}");
				}
			}
			Err(e) => {
				warn!("Error receiving datum from subscription: {e}");
			}
		}
	}

	async fn connect(&self, peer: EndpointAddr) -> Result<Subscription, Error> {
		debug!("Connecting to peer {peer:?} for stream {}", self.stream_id);
		let connection = self.local.connect(peer, Streams::ALPN_STREAMS).await?;

		let (wire_tx, wire_rx) = connection.open_bi().await?;
		let mut wire = Framed::new(
			tokio::io::join(wire_rx, wire_tx),
			LengthDelimitedCodec::new(),
		);

		// send subscription request
		wire
			.send(
				rmp_serde::to_vec(&SubscriptionRequest {
					stream_id: self.stream_id.clone(),
					criteria: self.criteria.clone(),
				})
				.expect("infallible")
				.into(),
			)
			.await?;

		let Some(response) = wire.next().await.transpose()? else {
			return Err(
				std::io::Error::new(
					std::io::ErrorKind::UnexpectedEof,
					"Connection closed before subscription response",
				)
				.into(),
			);
		};

		let Ok(response) = rmp_serde::from_slice::<SubscriptionResponse>(&response)
		else {
			return Err(
				std::io::Error::new(
					std::io::ErrorKind::InvalidData,
					"Invalid subscription response",
				)
				.into(),
			);
		};

		match response {
			SubscriptionResponse::Accepted => {
				info!(%self.stream_id, ?self.criteria, "Subscription accepted by {}", connection.remote_id()?);
			}
			SubscriptionResponse::Rejected(error) => {
				warn!(%self.stream_id, ?self.criteria, "Subscription rejected: {error}");
				connection.close(0u32.into(), b"bye!");
				let reason = connection.closed().await;
				info!(%self.stream_id, ?self.criteria, ?reason, "Connection closed");
				return Err(error.into());
			}
		}

		Ok(Subscription { connection, wire })
	}
}

#[derive(Debug)]
struct Subscription {
	pub connection: Connection,
	pub wire: Framed<Join<RecvStream, SendStream>, LengthDelimitedCodec>,
}

impl Stream for Subscription {
	type Item = Result<BytesMut, io::Error>;

	fn poll_next(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Self::Item>> {
		self.get_mut().wire.poll_next_unpin(cx)
	}
}
