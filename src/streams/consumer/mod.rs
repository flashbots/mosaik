//! Stream Consumers

use {
	super::Datum,
	core::{
		pin::Pin,
		task::{Context, Poll},
	},
	futures::Stream,
	tokio::sync::mpsc,
	tokio_util::sync::DropGuard,
};

mod builder;
mod receiver;
mod status;
mod worker;

pub use {builder::Builder, status::Status};

/// A local stream consumer handle that allows receiving data from a stream
/// produced by a remote peer.
///
/// Notes:
/// - Multiple [`Consumer`] instances can be created for the same stream id and
///   data type `D`. They will each receive their own copy of the data sent to
///   the stream.
pub struct Consumer<D> {
	status: Status,
	chan: mpsc::UnboundedReceiver<D>,
	_abort: DropGuard,
}

impl<D> Consumer<D> {
	/// Access to the consumer's status information.
	pub const fn status(&self) -> &Status {
		&self.status
	}
}

impl<D> Stream for Consumer<D> {
	type Item = D;

	fn poll_next(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Self::Item>> {
		self.get_mut().chan.poll_recv(cx)
	}
}
