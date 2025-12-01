use {
	super::{super::Streams, Datum, FanoutSink, Producer},
	crate::StreamId,
	dashmap::{Entry, mapref::one::Ref},
};

pub struct Builder<'s, D: Datum> {
	streams: &'s Streams,
	_marker: std::marker::PhantomData<D>,
}

impl<'s, D: Datum> Builder<'s, D> {
	/// Creates a new Producer Builder
	pub(crate) fn new(streams: &'s Streams) -> Self {
		Self {
			streams,
			_marker: std::marker::PhantomData,
		}
	}
}

/// Public API
impl<D: Datum> Builder<'_, D> {
	pub async fn build(&self) -> Producer<D> {
		let _sink = self.open_sink().await;
		Producer(std::marker::PhantomData)
	}
}

/// Internal methods
impl<D: Datum> Builder<'_, D> {
	/// Opens or creates the shared sink for this producer.
	///
	/// All producers of the same stream type share a single sink instance
	/// to fan out data to remote consumers. When a new sink is created, we
	/// also update our local node discovery entry to advertise the new stream to
	/// peers.
	async fn open_sink(&self) -> Ref<'_, StreamId, FanoutSink> {
		let stream_id = D::stream_id();
		match self.streams.sinks.entry(stream_id) {
			Entry::Vacant(entry) => {
				let sink = entry.insert(FanoutSink);

				self
					.streams
					.discovery
					.update_local_entry(move |me| me.add_streams(stream_id))
					.await;

				sink.downgrade()
			}
			Entry::Occupied(entry) => entry.into_ref().downgrade(),
		}
	}
}
