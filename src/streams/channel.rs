use tokio::sync::mpsc::{self, error::SendError};

pub struct Channel<T, const BACKLOG: usize = 32> {
	pub sender: mpsc::Sender<T>,
	pub receiver: mpsc::Receiver<T>,
}

impl<T, const BACKLOG: usize> Default for Channel<T, BACKLOG> {
	fn default() -> Self {
		let (sender, receiver) = mpsc::channel(BACKLOG);
		Self { sender, receiver }
	}
}
impl<T, const BACKLOG: usize> Channel<T, BACKLOG> {
	pub fn sender(&self) -> &mpsc::Sender<T> {
		&self.sender
	}

	pub fn receiver(&mut self) -> &mut mpsc::Receiver<T> {
		&mut self.receiver
	}

	pub async fn send(&self, message: T) -> Result<(), SendError<T>> {
		self.sender.send(message).await
	}

	pub async fn recv(&mut self) -> Option<T> {
		self.receiver.recv().await
	}
}

pub struct UnboundedChannel<T> {
	pub sender: mpsc::UnboundedSender<T>,
	pub receiver: mpsc::UnboundedReceiver<T>,
}

impl<T> Default for UnboundedChannel<T> {
	fn default() -> Self {
		let (sender, receiver) = mpsc::unbounded_channel();
		Self { sender, receiver }
	}
}
impl<T> UnboundedChannel<T> {
	pub fn sender(&self) -> &mpsc::UnboundedSender<T> {
		&self.sender
	}

	pub fn receiver(&mut self) -> &mut mpsc::UnboundedReceiver<T> {
		&mut self.receiver
	}

	pub fn send(&self, message: T) -> Result<(), SendError<T>> {
		self.sender.send(message)
	}

	pub async fn recv(&mut self) -> Option<T> {
		self.receiver.recv().await
	}
}
