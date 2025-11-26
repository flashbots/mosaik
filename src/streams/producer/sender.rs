use {
	super::{PublishError, Status},
	crate::prelude::{Datum, Network, StreamId},
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
	stream_id: StreamId,
	status: Arc<Status>,
	data_tx: PollSender<D>,
}

/// Public API
impl<D: Datum> Producer<D> {
	/// Creates a new producer for the given datum type on the provided network.
	///
	/// If this network already has a producer for this datum type, the created
	/// instance will reuse the existing producer and share its state.
	pub fn new(network: &Network) -> Self {
		network.local().create_sink::<D>().producer::<D>()
	}

	/// Access to the status of this producer.
	///
	/// The returned value can be used to query snapshots of statistics about
	/// the producer, as well as to await important status changes.
	pub fn status(&self) -> &Status {
		&self.status
	}

	/// Returns the stream ID for the datum type produced by this producer.
	pub const fn stream_id(&self) -> &StreamId {
		&self.stream_id
	}
}

/// Internal API
impl<D: Datum> Producer<D> {
	pub(crate) fn init(data_tx: mpsc::Sender<D>, status: Arc<Status>) -> Self {
		let stream_id = StreamId::of::<D>();
		let data_tx = PollSender::new(data_tx);

		Self {
			stream_id,
			status,
			data_tx,
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
