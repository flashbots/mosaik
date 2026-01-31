use tokio::sync::mpsc;

/// A simple unbounded channel wrapper around Tokio's mpsc unbounded channel.
/// This is used internally for various message passing needs to hold the sender
/// and receiver together.
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
	#[allow(dead_code)]
	pub const fn sender(&self) -> &mpsc::UnboundedSender<T> {
		&self.sender
	}

	#[allow(dead_code)]
	pub const fn receiver(&mut self) -> &mut mpsc::UnboundedReceiver<T> {
		&mut self.receiver
	}

	pub fn send(&self, message: T) {
		let _ = self.sender.send(message);
	}

	pub async fn recv(&mut self) -> Option<T> {
		self.receiver.recv().await
	}

	/// Checks if the receiver has no messages pending.
	pub fn is_empty(&self) -> bool {
		self.receiver.is_empty()
	}
}
