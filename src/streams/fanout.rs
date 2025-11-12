use {
	super::{
		Criteria,
		Datum,
		Error,
		Producer,
		StreamId,
		channel::Channel,
		protocol::Link,
	},
	crate::prelude::PeerId,
	bytes::Bytes,
	core::{
		any::Any,
		sync::atomic::{AtomicU32, Ordering},
	},
	futures::{SinkExt, StreamExt, stream::FuturesUnordered},
	std::{collections::HashMap, sync::Arc},
	tokio::sync::{Notify, mpsc},
	tokio_util::sync::{CancellationToken, DropGuard},
	tracing::info,
};

/// A handle to a fanout loop for a specific stream on the local node.
///
/// Every stream produced by the local node has a corresponding async fanout
/// loop task that is responsible for distributing data produced locally to all
/// remote subscribers according to their subscription criteria.
///
/// Notes:
///
/// - All local stream producers for a given stream ID share the same fanout
///   loop.
///
/// - Remote subscribers to a given stream ID will receive data produced by all
///   instances of `Producer<D>` on the local node for that stream ID.
///
/// - This type is just a handle to the fanout loop; it is safe and cheap to
///   clone. All clones of this type control the same underlying fanout loop
///   task.
pub struct FanoutSink(Arc<Inner>);

impl Clone for FanoutSink {
	fn clone(&self) -> Self {
		Self(Arc::clone(&self.0))
	}
}

impl FanoutSink {
	/// Creates a new `FanoutSink` for the given stream ID with no producers or
	/// remote subscribers.
	pub fn new<D: Datum>() -> Self {
		let stream_id = StreamId::of::<D>();
		let event_loop = EventLoop::<D>::default();

		let inner = Arc::new(Inner {
			stream_id,
			_abort: event_loop.cancel.clone().drop_guard(),
			cmd_tx: event_loop.cmds.sender().clone(),
			data_tx: Box::new(event_loop.data.sender().clone()),
			subs_count: Arc::clone(&event_loop.subs_count),
			ready: Arc::clone(&event_loop.ready),
		});

		tokio::spawn(event_loop.run());

		Self(inner)
	}

	/// Returns the stream ID associated with this fanout sink.
	pub fn stream_id(&self) -> &StreamId {
		&self.0.stream_id
	}

	/// Creates a new producer instance for this fanout sink.
	///
	/// There may be multiple producers for the same stream ID on the local node;
	/// all such producers will share the same fanout loop for distributing data
	/// to remote subscribers.
	pub fn producer<D: Datum>(&self) -> Result<Producer<D>, Error> {
		let datum_stream_id = StreamId::of::<D>();
		if datum_stream_id != *self.stream_id() {
			return Err(Error::MismatchedStreamId {
				expected: self.stream_id().clone(),
				found: datum_stream_id,
			});
		}

		let data_tx = &self.0.data_tx;
		let data_tx = data_tx
			.downcast_ref::<mpsc::Sender<D>>()
			.ok_or(Error::MismatchedStreamId {
				expected: self.stream_id().clone(),
				found: datum_stream_id,
			})?
			.clone();

		Ok(Producer::new(
			data_tx,
			Arc::clone(&self.0.subs_count),
			Arc::clone(&self.0.ready),
		))
	}

	pub async fn accept(
		&self,
		link: Link,
		criteria: Criteria,
	) -> Result<(), Error> {
		self
			.0
			.cmd_tx
			.send(LoopCommand::Accept(link, criteria))
			.await
			.map_err(|_| Error::Terminated)
	}
}

struct Inner {
	stream_id: StreamId,
	cmd_tx: mpsc::Sender<LoopCommand>,
	data_tx: Box<dyn Any + Send + Sync>,
	subs_count: Arc<AtomicU32>,
	ready: Arc<Notify>,
	_abort: DropGuard,
}

struct EventLoop<D: Datum> {
	data: Channel<D>,
	cmds: Channel<LoopCommand>,
	cancel: CancellationToken,
	subs_count: Arc<AtomicU32>,
	ready: Arc<Notify>,
	stream_id: StreamId,
	subs: HashMap<PeerId, (Link, Criteria)>,
}

impl<D: Datum> Default for EventLoop<D> {
	fn default() -> Self {
		Self {
			cancel: CancellationToken::new(),
			cmds: Channel::default(),
			data: Channel::default(),
			subs_count: Arc::new(AtomicU32::new(0)),
			stream_id: StreamId::of::<D>(),
			subs: HashMap::new(),
			ready: Arc::new(Notify::new()),
		}
	}
}

impl<D: Datum> EventLoop<D> {
	pub async fn run(mut self) -> Result<(), Error> {
		loop {
			tokio::select! {
				_ = self.cancel.cancelled() => {
					self.on_terminated().await;
					break Ok(());
				}
				Some(cmd) = self.cmds.receiver().recv() => self.on_command(cmd).await?,
				Some(item) = self.data.receiver().recv() => self.on_data(item).await?,
			}
		}
	}

	async fn on_command(&mut self, cmd: LoopCommand) -> Result<(), Error> {
		info!(
			"Received command {cmd:?} for fanout loop of stream {}",
			self.stream_id
		);

		match cmd {
			LoopCommand::Accept(link, criteria) => {
				self.on_new_subscriber(link, criteria).await
			}
		}
	}

	async fn on_new_subscriber(
		&mut self,
		link: Link,
		criteria: Criteria,
	) -> Result<(), Error> {
		let mut link = link;
		let peer_id = link.connection.remote_id().unwrap();

		info!(
			"New subscriber added to fanout loop for stream {}. Total subscribers: \
			 {}",
			self.stream_id,
			self.subscribers_count()
		);

		link
			.wire
			.send(
				rmp_serde::to_vec(&super::protocol::SubscriptionResponse::Accepted)
					.expect("infallible; qed")
					.into(),
			)
			.await?;

		self.subs.insert(peer_id, (link, criteria));
		if self.subs_count.fetch_add(1, Ordering::Relaxed) == 0 {
			self.ready.notify_waiters();
		}

		Ok(())
	}

	async fn on_data(&mut self, _item: D) -> Result<(), Error> {
		info!(
			"Publishing item to {} subscribers for stream {}",
			self.subscribers_count(),
			self.stream_id
		);

		let bytes: Bytes =
			rmp_serde::to_vec(&_item).expect("infallible; qed").into();

		let mut tasks = FuturesUnordered::new();
		for (_peer_id, (link, _)) in self.subs.iter_mut() {
			tasks.push(link.wire.send(bytes.clone()));
		}

		while let Some(result) = tasks.next().await {
			match result {
				Ok(_) => {
					info!(
						"Successfully sent data to subscriber in fanout loop for stream {}",
						self.stream_id
					);
				}
				Err(e) => {
					info!(
						"Error sending data to subscriber in fanout loop for stream {}: \
						 {e:?}",
						self.stream_id
					);
				}
			}
		}

		Ok(())
	}

	async fn on_terminated(&mut self) {
		info!("Fanout loop for stream {} is terminating", self.stream_id);
	}
}

impl<D: Datum> EventLoop<D> {
	fn subscribers_count(&self) -> u32 {
		self.subs_count.load(Ordering::Relaxed)
	}
}

#[derive(Debug)]
enum LoopCommand {
	Accept(Link, Criteria),
}

#[cfg(test)]
mod tests {
	use {
		super::*,
		crate::streams::producer::PublishError,
		futures::SinkExt,
		serde::{Deserialize, Serialize},
	};

	#[derive(Debug, PartialEq, Serialize, Deserialize)]
	struct Data1(pub String);

	#[tokio::test]
	async fn error_on_no_consumers() {
		let fanout = FanoutSink::new::<Data1>();
		let mut producer = fanout.producer::<Data1>().unwrap();

		let result = producer.send(Data1("test".to_string())).await;

		assert_eq!(
			result,
			Err(PublishError::NoConsumers(Data1("test".to_string())))
		);
	}

	#[tokio::test]
	async fn one_subscriber_receives_data() {
		let fanout = FanoutSink::new::<Data1>();
		let mut producer = fanout.producer::<Data1>().unwrap();

		producer.send(Data1("test".to_string())).await.unwrap();
	}
}
