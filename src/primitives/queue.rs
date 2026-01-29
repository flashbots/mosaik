use {
	core::{
		future::pending,
		pin::Pin,
		task::{Context, Poll},
	},
	futures::{Stream, StreamExt, stream::FuturesUnordered},
};

/// An asynchronous work queue that manages a set of futures to be executed
/// concurrently. This is often used in worker loops to enqueue async tasks that
/// need to be processed without blocking the main event loop.
pub struct AsyncWorkQueue<T: Send + Sync + 'static = ()>(
	FuturesUnordered<
		Pin<Box<dyn futures::Future<Output = T> + Send + Sync + 'static>>,
	>,
);

impl<T: Send + Sync + 'static> Default for AsyncWorkQueue<T> {
	fn default() -> Self {
		Self::new()
	}
}

impl<T: Send + Sync + 'static> AsyncWorkQueue<T> {
	/// Creates a new, empty `AsyncWorkQueue`.
	pub fn new() -> Self {
		let inner = FuturesUnordered::<
			Pin<Box<dyn futures::Future<Output = T> + Send + Sync + 'static>>,
		>::new();
		inner.push(Box::pin(pending::<T>()));
		Self(inner)
	}

	/// Adds a new future to the work queue.
	pub fn enqueue<F>(&mut self, fut: F)
	where
		F: Future<Output = T> + Send + Sync + 'static,
	{
		self.0.push(Box::pin(fut));
	}
}

impl<T: Send + Sync + 'static> Stream for AsyncWorkQueue<T> {
	type Item = T;

	fn poll_next(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Self::Item>> {
		self.0.poll_next_unpin(cx)
	}
}
