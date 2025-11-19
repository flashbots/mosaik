use {
	super::{super::Datum, *},
	crate::prelude::Network,
	core::{
		pin::Pin,
		task::{Context, Poll},
	},
	futures::{Sink, SinkExt},
	std::sync::Arc,
	tokio::sync::mpsc,
	tokio_util::sync::PollSender,
};

pub struct Producer<D: Datum> {
	status: Arc<Status>,
	data_tx: PollSender<D>,
}

/// Public API
impl<D: Datum> Producer<D> {
	/// Access to the status of this producer.
	///
	/// The returned value can be used to query snapshots of statistics about
	/// the producer, as well as to await important status changes.
	pub fn status(&self) -> &Status {
		&self.status
	}

	/// Creates a new producer for the given datum type on the provided network.
	///
	/// If this network already has a producer for this datum type, the created
	/// instance will reuse the existing producer and share its state.
	pub fn new(network: &Network) -> Self {
		network.local().create_sink::<D>().producer::<D>()
	}
}

/// Internal API
impl<D: Datum> Producer<D> {
	pub(crate) fn init(data_tx: mpsc::Sender<D>, status: Arc<Status>) -> Self {
		let data_tx = PollSender::new(data_tx);

		Self { data_tx, status }
	}
}

impl<D: Datum> Sink<D> for Producer<D> {
	type Error = PublishError<D>;

	fn poll_ready(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Result<(), Self::Error>> {
		self
			.get_mut()
			.data_tx
			.poll_ready_unpin(cx)
			.map_err(|_| PublishError::Terminated)
	}

	fn start_send(self: Pin<&mut Self>, item: D) -> Result<(), Self::Error> {
		if self.status().subscribers_count() == 0 {
			self.get_mut().data_tx.abort_send();
			return Err(PublishError::NoConsumers(item));
		}

		self.get_mut().data_tx.start_send_unpin(item).map_err(|e| {
			match e.into_inner() {
				Some(e) => PublishError::NoConsumers(e),
				_ => PublishError::Terminated,
			}
		})
	}

	fn poll_flush(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Result<(), Self::Error>> {
		self
			.get_mut()
			.data_tx
			.poll_flush_unpin(cx)
			.map_err(|_| PublishError::Terminated)
	}

	fn poll_close(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Result<(), Self::Error>> {
		self
			.get_mut()
			.data_tx
			.poll_close_unpin(cx)
			.map_err(|_| PublishError::Terminated)
	}
}

#[cfg(test)]
mod tests {
	use {
		super::*,
		core::sync::atomic::Ordering,
		futures::{future::poll_fn, SinkExt},
		serde::{Deserialize, Serialize},
	};

	#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
	struct DummyDatum(String);

	impl DummyDatum {
		fn new(value: &str) -> Self {
			Self(value.to_string())
		}
	}

	#[tokio::test]
	async fn start_send_errors_without_subscribers() {
		let (tx, mut rx) = mpsc::channel::<DummyDatum>(1);
		let status = Arc::new(Status::new());
		let mut producer = Producer::init(tx, Arc::clone(&status));

		let err = producer.send(DummyDatum::new("one")).await.unwrap_err();
		match err {
			PublishError::NoConsumers(d) => assert_eq!(d, DummyDatum::new("one")),
			other => panic!("unexpected error {other:?}"),
		}

		assert!(rx.try_recv().is_err());
	}

	#[tokio::test]
	async fn send_succeeds_with_subscriber() {
		let (tx, mut rx) = mpsc::channel::<DummyDatum>(1);
		let status = Arc::new(Status::new());
		status.subscribers_count.store(1, Ordering::Relaxed);
		let mut producer = Producer::init(tx, Arc::clone(&status));

		producer.send(DummyDatum::new("ok")).await.unwrap();
		assert_eq!(rx.recv().await, Some(DummyDatum::new("ok")));
	}

	#[tokio::test]
	async fn poll_ready_errors_after_channel_closed() {
		let (tx, rx) = mpsc::channel::<DummyDatum>(1);
		let status = Arc::new(Status::new());
		let mut producer = Producer::init(tx, Arc::clone(&status));
		drop(rx);

		let result = poll_fn(|cx| Pin::new(&mut producer).poll_ready(cx)).await;
		assert!(matches!(result, Err(PublishError::Terminated)));
	}
}
