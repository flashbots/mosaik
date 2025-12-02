use {
	super::{
		super::{Config, Criteria, Datum, StreamId},
		Producer,
		Status,
	},
	crate::network::link::Link,
	core::any::Any,
	std::sync::Arc,
	tokio::sync::mpsc,
};

/// Fanout Sink handle for a specific stream.
///
/// This structs provides an interface to interact with the long-running fanout
/// sink worker loop for a specific stream id.
pub(in crate::streams) struct Handle {
	/// The unique stream id associated with this sink.
	///
	/// This type is bound to the specific datum type `D` used to create the
	/// sink and is used to validate type parameters when casting the sender
	/// channel.
	stream_id: StreamId,

	/// A type erased sender channel for sending datum to the sink worker loop.
	///
	/// This is type erased to allow handles to be stored in a heterogeneous map
	/// such as `Sinks`.
	data_tx: Box<dyn Any + Send + Sync>,
}

impl Handle {
	/// Returns a typed sender channel for sending datum of type `D` to the
	/// sink worker loop.
	pub fn sender<D: Datum>(&self) -> Producer<D> {
		// Validate that the stream id matches the expected datum type
		assert_eq!(
			self.stream_id,
			D::stream_id(),
			"StreamId mismatch: expected {}, got {}",
			self.stream_id,
			D::stream_id()
		);

		// Downcast the type erased sender channel to the expected type
		let data_tx = self
			.data_tx
			.downcast_ref::<mpsc::Sender<D>>()
			.expect("Failed to downcast data sender channel");

		Producer::new(data_tx.clone(), Status)
	}

	/// Accepts an incoming connection from a remote consumer for this stream id.
	///
	/// By the time the connection is accepted, the [`Acceptor`] has already
	/// decoded the handshake message and opened a transport-level stream with
	/// the remote peer.
	pub async fn accept(&self, _: Link, _: Criteria) {
		async {}.await;
	}
}

/// Fanout Sink background worker loop
///
/// This is a long-running task that is owned by the [`Sinks`] struct that
/// manages all active fanout sinks for producers.
///
/// There is an instance of this struct for each active stream id that has
/// at least one producer associated with it.
pub(super) struct WorkerLoop<D: Datum> {
	config: Arc<Config>,
	data_rx: mpsc::Receiver<D>,
}

impl<D: Datum> WorkerLoop<D> {
	/// Spawns a new fanout sink worker loop for the given stream id.
	pub(super) fn spawn(config: Arc<Config>) -> Handle {
		let stream_id = D::stream_id();
		let (data_tx, data_rx) = mpsc::channel::<D>(config.producer_buffer_size);

		let worker = WorkerLoop { config, data_rx };
		tokio::spawn(worker.run());

		Handle {
			stream_id,
			data_tx: Box::new(data_tx),
		}
	}
}

impl<D: Datum> WorkerLoop<D> {
	pub async fn run(self) {
		async {}.await;
		todo!()
	}
}
