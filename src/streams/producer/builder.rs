use {
	super::{
		super::{Datum, Streams},
		Producer,
	},
	crate::{NetworkId, discovery::PeerEntry},
	core::marker::PhantomData,
};

#[derive(Debug, thiserror::Error)]
pub enum Error<D: Datum> {
	/// A producer for the given datum type already exists.
	///
	/// This error is returned when attempting to create a new producer
	/// through the builder while one already exists for the same datum type.
	///
	/// If you need multiple producers for the same datum type, consider using
	/// the default `produce` method which allows multiple instances to
	/// share the same underlying stream.
	#[error("Producer for datum type already exists")]
	AlreadyExists(Producer<D>),
}

pub(in crate::streams) struct ProducerConfig {
	/// The buffer size for the producer's internal channel that holds datum
	/// before they are sent to connected consumers. If the buffer is full, calls
	/// to `send` on the producer will await until there is space available.
	pub buffer_size: usize,

	/// Sets a predicate function that is used to determine whether to
	/// accept or reject incoming consumer connections.
	pub accept_if: Box<dyn FnMut(&PeerEntry) -> bool + Send + Sync>,

	/// The network id this producer is associated with.
	pub network_id: NetworkId,
}

/// Configurable builder for assembling a new producer instances for a specific
/// datum type `D`.
pub struct Builder<'s, D: Datum> {
	config: ProducerConfig,
	streams: &'s Streams,
	_marker: PhantomData<D>,
}

/// Public API
impl<D: Datum> Builder<'_, D> {
	/// Sets a predicate function that is used to determine whether to
	/// accept or reject incoming consumer connections.
	#[must_use]
	pub fn accept_if<F>(mut self, pred: F) -> Self
	where
		F: FnMut(&PeerEntry) -> bool + Send + Sync + 'static,
	{
		self.config.accept_if = Box::new(pred);
		self
	}

	/// Sets the buffer size for the producer's internal channel that holds datum
	/// before they are sent to connected consumers. If the buffer is full, calls
	/// to `send` on the producer will await until there is space available.
	#[must_use]
	pub fn with_buffer_size(mut self, size: usize) -> Self {
		self.config.buffer_size = size;
		self
	}

	/// Builds a new producer with the given configuration for this stream id.
	/// If there is already an existing producer for this stream id, an error
	/// is returned containing the existing producer created using the original
	/// configuration.
	pub fn build(self) -> Result<Producer<D>, Error<D>> {
		if let Some(existing) = self.streams.sinks.open(D::stream_id()) {
			return Err(Error::AlreadyExists(existing.sender()));
		}

		self
			.streams
			.sinks
			.create::<D>(self.config)
			.map(|handle| handle.sender())
			.map_err(|existing| Error::AlreadyExists(existing.sender()))
	}
}

impl<'s, D: Datum> Builder<'s, D> {
	pub(in crate::streams) fn new(streams: &'s Streams) -> Self {
		Self {
			streams,
			config: ProducerConfig {
				buffer_size: 1024,
				accept_if: Box::new(|_| true),
				network_id: *streams.local.network_id(),
			},
			_marker: PhantomData,
		}
	}
}
