use {
	super::{
		Datum,
		error::PublishError,
		fanout::{FanoutCommand, FanoutHandle},
	},
	core::{
		pin::Pin,
		task::{Context, Poll},
	},
	futures::Sink,
	tokio::sync::mpsc::{Permit, error::SendError},
	tokio_util::sync::ReusableBoxFuture,
};

pub struct Producer<D: Datum> {
	handle: FanoutHandle<D>,
	permit: Option<Permit<'static, FanoutCommand<D>>>,
	reserve_fut: ReusableBoxFuture<
		'static,
		Result<Permit<'static, FanoutCommand<D>>, SendError<()>>,
	>,
	fut_initialized: bool,
}

impl<D: Datum> Producer<D> {
	pub(crate) fn new(handle: FanoutHandle<D>) -> Self {
		// Initialize with a dummy future
		let reserve_fut = ReusableBoxFuture::new(async { Err(SendError(())) });

		Self {
			handle,
			permit: None,
			reserve_fut,
			fut_initialized: false,
		}
	}

	/// Publishes an item to all downstream consumers.
	///
	/// Notes:
	pub async fn publish(&mut self, item: D) -> Result<(), PublishError<D>> {
		self.handle.publish(item).await
	}
}

impl<D: Datum> Sink<D> for Producer<D> {
	type Error = PublishError<D>;

	/// Checks if the sink is ready to accept more data.
	///
	/// This method handles backpressure by reserving a permit from the underlying
	/// mpsc channel. If the channel is full, this will return `Poll::Pending` and
	/// register the task to be woken when capacity becomes available.
	///
	/// The reserved permit is stored and used in `start_send` to ensure that
	/// sending never blocks or fails due to a full channel.
	fn poll_ready(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Result<(), Self::Error>> {
		if self.permit.is_some() {
			return Poll::Ready(Ok(()));
		}

		let this = self.get_mut();

		// Only set the future if it hasn't been initialized
		if !this.fut_initialized {
			// SAFETY: We need to extend the lifetime of the sender to 'static
			// This is safe because the sender is owned by handle which is owned by
			// this Producer
			let sender = unsafe {
				core::mem::transmute::<
					&tokio::sync::mpsc::Sender<FanoutCommand<D>>,
					&'static tokio::sync::mpsc::Sender<FanoutCommand<D>>,
				>(this.handle.cmd_sender())
			};

			// Set up the reserve future
			this.reserve_fut.set(async move { sender.reserve().await });
			this.fut_initialized = true;
		}

		match this.reserve_fut.poll(cx) {
			Poll::Ready(Ok(permit)) => {
				this.permit = Some(permit);
				this.fut_initialized = false; // Reset for next time
				Poll::Ready(Ok(()))
			}
			Poll::Ready(Err(_)) => {
				this.fut_initialized = false; // Reset on error
				Poll::Ready(Err(PublishError::Terminated))
			}
			Poll::Pending => Poll::Pending,
		}
	}

	fn start_send(mut self: Pin<&mut Self>, item: D) -> Result<(), Self::Error> {
		// this permit will be released anyway at the end of this method.
		let permit = self
			.permit
			.take()
			.expect("start_send called without poll_ready");

		if self.handle.subscribers_count() == 0 {
			return Err(PublishError::NoConsumers(item));
		}

		permit.send(FanoutCommand::Publish(item));
		Ok(())
	}

	fn poll_flush(
		self: Pin<&mut Self>,
		_: &mut Context<'_>,
	) -> Poll<Result<(), Self::Error>> {
		Poll::Ready(Ok(()))
	}

	fn poll_close(
		self: Pin<&mut Self>,
		_: &mut Context<'_>,
	) -> Poll<Result<(), Self::Error>> {
		Poll::Ready(Ok(()))
	}
}
