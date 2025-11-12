use {
	super::Datum,
	core::{
		pin::Pin,
		sync::atomic::{AtomicU32, Ordering},
		task::{Context, Poll},
	},
	futures::{Sink, SinkExt},
	std::sync::Arc,
	tokio::sync::{Notify, mpsc},
	tokio_util::sync::PollSender,
};

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum PublishError<D: Datum> {
	#[error("stream has no consumers")]
	NoConsumers(D),

	#[error("stream terminated")]
	Terminated,
}

pub struct Producer<D: Datum> {
	subs_count: Arc<AtomicU32>,
	ready: Arc<Notify>,
	data_tx: PollSender<D>,
}

impl<D: Datum> Producer<D> {
	pub(crate) fn new(
		data_tx: mpsc::Sender<D>,
		subs_count: Arc<AtomicU32>,
		ready: Arc<Notify>,
	) -> Self {
		let data_tx = PollSender::new(data_tx);
		Self {
			data_tx,
			subs_count,
			ready,
		}
	}

	pub fn subscribers_count(&self) -> u32 {
		self.subs_count.load(Ordering::Relaxed)
	}

	/// Awaits until the producer is ready to produce data and has at least one
	/// subscriber.
	pub async fn online(&self) {
		if self.subscribers_count() == 0 {
			self.ready.notified().await;
		}
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
		if self.subscribers_count() == 0 {
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
