use {
	core::task::{Context, Poll},
	tokio::sync::mpsc,
};

/// A simple unbounded channel wrapper around Tokio's mpsc unbounded channel.
/// This is used internally for various message passing needs to hold the sender
/// and receiver together.
#[derive(Debug)]
pub struct UnboundedChannel<T> {
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

	/// Returns the number of messages currently pending in the receiver.
	pub fn len(&self) -> usize {
		self.receiver.len()
	}

	/// Polls the channel for available messages.
	///
	/// This is used when the channel is polled inside a `Future` implementation
	/// to check for new messages without blocking.
	#[allow(dead_code)]
	pub fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<T>> {
		self.receiver.poll_recv(cx)
	}

	/// Polls the channel for multiple available messages at once.
	///
	/// This is a convenience method that repeatedly polls the channel until no
	/// more messages are available, collecting them into a vector. This can be
	/// more efficient than polling for one message at a time when we expect
	/// multiple messages to arrive together. Returns `Poll::Ready(Vec<T>)` if
	/// one or more messages are available or `Poll::Pending` if no messages are
	/// currently available.
	pub fn poll_recv_many(
		&mut self,
		cx: &mut Context<'_>,
		buffer: &mut Vec<T>,
		limit: usize,
	) -> Poll<usize> {
		self.receiver.poll_recv_many(cx, buffer, limit)
	}
}
