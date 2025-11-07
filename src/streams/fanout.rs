use {
	super::{Datum, Error, Producer, error::PublishError},
	crate::streams::{Criteria, StreamId},
	core::{
		any::Any,
		pin::Pin,
		sync::atomic::{AtomicU32, Ordering},
		task::{Context, Poll},
	},
	futures::FutureExt,
	iroh::endpoint::Connection,
	std::sync::Arc,
	tokio::{
		sync::mpsc::{self, error::SendError},
		task::JoinHandle,
	},
	tokio_util::{
		codec::{Framed, LengthDelimitedCodec},
		sync::{CancellationToken, DropGuard},
	},
	tracing::info,
};

pub struct Fanout {
	id: StreamId,
	handle: Box<dyn Any + Send + Sync>,
	worker: JoinHandle<Result<(), Error>>,
	_cancel_on_drop: DropGuard,
}

impl Fanout {
	pub fn new<D: Datum>() -> Self {
		let id = StreamId::of::<D>();
		let process = FanoutProcess::<D>::new();
		let handle = Box::new(process.handle());
		let _cancel_on_drop = process.cancel_on_drop();
		let worker = tokio::spawn(process.run());

		Self {
			id,
			handle,
			worker,
			_cancel_on_drop,
		}
	}

	pub(crate) fn handle<D: Datum>(&self) -> FanoutHandle<D> {
		self
			.handle
			.downcast_ref::<FanoutHandle<D>>()
			.expect("Fanout handle type mismatch")
			.clone()
	}

	pub fn producer<D: Datum>(&self) -> Producer<D> {
		Producer::new(self.handle::<D>())
	}

	pub async fn subscribe(&self, sub: Subscription) -> Result<(), Error> {
		let id = self.id.clone();

		info!(
			"Fanout for stream {id} has new subscriber from {} with criteria {:?}",
			sub.connection.remote_id().unwrap(),
			sub.criteria
		);

		Ok(())
	}
}

impl Future for Fanout {
	type Output = Result<(), Error>;

	fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
		let Poll::Ready(result) = self.get_mut().worker.poll_unpin(cx) else {
			return Poll::Pending;
		};

		match result {
			Ok(result) => Poll::Ready(result),
			Err(_) => Poll::Ready(Err(Error::Terminated)),
		}
	}
}

struct Runloop<D: Datum> {
	cancel: CancellationToken,
	subs_count: Arc<AtomicU32>,
	subs: Vec<Subscription>,
	cmd_tx: mpsc::Sender<FanoutCommand<D>>,
	cmd_rx: mpsc::Receiver<FanoutCommand<D>>,
}

struct FanoutProcess<D: Datum> {
	cancel: CancellationToken,
	subs_count: Arc<AtomicU32>,
	subs: Vec<Subscription>,
	cmd_tx: mpsc::Sender<FanoutCommand<D>>,
	cmd_rx: mpsc::Receiver<FanoutCommand<D>>,
}

impl<D: Datum> FanoutProcess<D> {
	pub fn new() -> Self {
		let cancel = CancellationToken::new();
		let (cmd_tx, cmd_rx) = mpsc::channel(16);
		let subs_count = Arc::new(AtomicU32::new(0));
		let subs = Vec::new();

		Self {
			cancel,
			subs_count,
			subs,
			cmd_tx,
			cmd_rx,
		}
	}

	pub fn cancel_on_drop(&self) -> DropGuard {
		self.cancel.clone().drop_guard()
	}

	pub fn subscribers_count(&self) -> u32 {
		self.subs_count.load(Ordering::Relaxed)
	}

	pub fn handle(&self) -> FanoutHandle<D> {
		FanoutHandle {
			subs_count: Arc::clone(&self.subs_count),
			cmd_tx: self.cmd_tx.clone(),
		}
	}
}

impl<D: Datum> FanoutProcess<D> {
	async fn run(mut self) -> Result<(), Error> {
		loop {
			tokio::select! {
					_ = self.cancel.cancelled() => {
						break Ok(());
					}
					Some(cmd) = self.cmd_rx.recv() => {
						match cmd {
							FanoutCommand::Publish(item) => {
								self.broadcast(item).await?;
							}
							FanoutCommand::Subscribe(sub) => {
								self.subs.push(sub);
								self.subs_count.fetch_add(1, Ordering::Relaxed);
								info!("New subscriber added, total subscribers: {}", self.subscribers_count());
							}
					}
				}
			}
		}
	}

	async fn broadcast(&mut self, _item: D) -> Result<(), Error> {
		info!(
			"Publishing item to {} subscribers",
			self.subscribers_count()
		);
		Ok(())
	}
}

pub struct FanoutHandle<D: Datum> {
	subs_count: Arc<AtomicU32>,
	cmd_tx: mpsc::Sender<FanoutCommand<D>>,
}

impl<D: Datum> Clone for FanoutHandle<D> {
	fn clone(&self) -> Self {
		Self {
			subs_count: Arc::clone(&self.subs_count),
			cmd_tx: self.cmd_tx.clone(),
		}
	}
}

impl<D: Datum> FanoutHandle<D> {
	pub fn subscribers_count(&self) -> u32 {
		self.subs_count.load(Ordering::Relaxed)
	}

	pub async fn publish(&self, item: D) -> Result<(), PublishError<D>> {
		if self.subscribers_count() == 0 {
			return Err(PublishError::NoConsumers(item));
		}

		let cmd = FanoutCommand::Publish(item);
		self.cmd_tx.send(cmd).await.map_err(|SendError(e)| {
			let FanoutCommand::Publish(item) = e else {
				unreachable!("must be Publish command; qed");
			};
			PublishError::NoConsumers(item)
		})?;

		Ok(())
	}
}

// internal API
impl<D: Datum> FanoutHandle<D> {
	pub(crate) const fn cmd_sender(&self) -> &mpsc::Sender<FanoutCommand<D>> {
		&self.cmd_tx
	}
}

pub enum FanoutCommand<D: Datum> {
	Publish(D),
	Subscribe(Subscription),
}

pub struct Subscription {
	pub connection: Connection,
	pub criteria: Criteria,
	pub wire: Framed<
		tokio::io::Join<iroh::endpoint::RecvStream, iroh::endpoint::SendStream>,
		LengthDelimitedCodec,
	>,
}

#[cfg(test)]
mod tests {
	use {
		super::*,
		serde::{Deserialize, Serialize},
	};

	#[derive(Debug, PartialEq, Serialize, Deserialize)]
	struct Data1(pub String);

	#[tokio::test]
	async fn error_on_no_consumers() {
		let fanout = Fanout::new::<Data1>();
		let handle = fanout.handle::<Data1>();

		let result = handle.publish(Data1("test".to_string())).await;

		assert_eq!(
			result,
			Err(PublishError::NoConsumers(Data1("test".to_string())))
		);
	}

	#[tokio::test]
	async fn one_subscriber_receives_data() {
		let fanout = Fanout::new::<Data1>();
		let handle = fanout.handle::<Data1>();

		handle.publish(Data1("test".to_string())).await.unwrap();
	}
}
