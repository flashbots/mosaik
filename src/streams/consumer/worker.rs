use {
	super::{super::Criteria, Consumer, Datum, Status},
	crate::{
		discovery::{Catalog, Discovery},
		network::LocalNode,
		streams,
	},
	tokio::sync::{mpsc, watch},
	tokio_util::sync::CancellationToken,
};

/// Worker task that manages receiving data for a stream consumer.
/// There is one worker task per consumer instance.
pub(super) struct ReceiveWorker<D: Datum> {
	/// The stream subscription criteria for this consumer.
	criteria: Criteria,

	/// Watch handle for the discovery catalog.
	///
	/// This is used to monitor known peers that are producing the desired
	/// stream.
	catalog: watch::Receiver<Catalog>,

	/// Channel for sending received data to the consumer handle.
	data_tx: mpsc::UnboundedSender<D>,

	/// Triggered when the consumer handle is dropped.
	cancel: CancellationToken,
}

impl<D: Datum> ReceiveWorker<D> {
	/// Spawns a new receive worker task and returns the consumer handle that can
	/// receive data and query status.
	///
	/// The worker handle will terminate when the returned consumer is dropped.
	pub fn spawn(
		local: &LocalNode,
		discovery: &Discovery,
		_config: &streams::Config,
		criteria: Criteria,
	) -> Consumer<D> {
		let cancel = local.termination().child_token();
		let (data_tx, data_rx) = mpsc::unbounded_channel();

		// Get a watch handle for the discovery catalog
		// and mark it as changed to trigger an catalog lookup.
		let mut catalog = discovery.catalog_watch();
		catalog.mark_changed();

		let worker = ReceiveWorker {
			data_tx,
			criteria,
			catalog,
			cancel: cancel.clone(),
		};

		tokio::spawn(worker.run());

		Consumer {
			status: Status,
			chan: data_rx,
			_abort: cancel.drop_guard(),
		}
	}
}

impl<D: Datum> ReceiveWorker<D> {
	async fn run(mut self) {
		loop {
			tokio::select! {
				// Triggered when the consumer is dropped or the network is shutting down
				() = self.cancel.cancelled() => {
					self.on_terminated().await;
					break;
				}

				// Triggered when new peers are discovered or existing peers are updated
				_ = self.catalog.changed() => {
					// mark the latest catalog snapshot as seen and trigger peers scan
					let snapshot = self.catalog.borrow_and_update().clone();
					self.on_catalog_update(snapshot);
				}
			}
		}
	}

	/// Handles updates to the discovery catalog.
	fn on_catalog_update(&mut self, latest: Catalog) {
		let stream_id = D::stream_id();

		let producers = latest
			.peers()
			.filter(|peer| peer.streams().contains(&stream_id));

		for producer in producers {
			tracing::info!(
				">--> Consumer for {} found producer: {:?}",
				stream_id,
				producer
			);
		}
	}

	/// Gracefully closes all connections with remote producers.
	async fn on_terminated(&mut self) {
		tracing::info!("Consumer worker for {} terminating", D::stream_id());
	}
}
