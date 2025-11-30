use tokio::sync::mpsc::{self, error::SendError};

pub(crate) struct Channel<T, const BACKLOG: usize = 32> {
	sender: mpsc::Sender<T>,
	receiver: mpsc::Receiver<T>,
}

impl<T, const BACKLOG: usize> Default for Channel<T, BACKLOG> {
	fn default() -> Self {
		let (sender, receiver) = mpsc::channel(BACKLOG);
		Self { sender, receiver }
	}
}

impl<T, const BACKLOG: usize> Channel<T, BACKLOG> {
	pub(crate) fn sender(&self) -> &mpsc::Sender<T> {
		&self.sender
	}

	#[allow(dead_code)]
	pub(crate) fn receiver(&mut self) -> &mut mpsc::Receiver<T> {
		&mut self.receiver
	}

	#[allow(dead_code)]
	pub(crate) async fn send(&self, message: T) -> Result<(), SendError<T>> {
		self.sender.send(message).await
	}

	pub(crate) async fn recv(&mut self) -> Option<T> {
		self.receiver.recv().await
	}
}

pub(crate) struct UnboundedChannel<T> {
	sender: mpsc::UnboundedSender<T>,
	receiver: mpsc::UnboundedReceiver<T>,
}

impl<T> Default for UnboundedChannel<T> {
	fn default() -> Self {
		let (sender, receiver) = mpsc::unbounded_channel();
		Self { sender, receiver }
	}
}

impl<T> UnboundedChannel<T> {
	pub(crate) fn sender(&self) -> &mpsc::UnboundedSender<T> {
		&self.sender
	}

	pub(crate) fn receiver(&mut self) -> &mut mpsc::UnboundedReceiver<T> {
		&mut self.receiver
	}

	pub(crate) fn send(&self, message: T) {
		self
			.sender
			.send(message)
			.expect("Receiver lifetime bound to sender lifetime");
	}

	pub(crate) async fn recv(&mut self) -> Option<T> {
		self.receiver.recv().await
	}
}
