use {
	super::{super::Streams, Consumer, Datum, worker},
	crate::Criteria,
	core::marker::PhantomData,
};

pub struct Builder<'s, D: Datum> {
	streams: &'s Streams,
	criteria: Criteria,
	_marker: PhantomData<D>,
}

impl<'s, D: Datum> Builder<'s, D> {
	/// Creates a new consumer builder instance with default criteria.
	pub(crate) fn new(streams: &'s Streams) -> Self {
		Self {
			streams,
			criteria: Criteria::default(),
			_marker: PhantomData,
		}
	}

	/// Sets the criteria for stream subscription.
	#[must_use]
	pub fn with_criteria(mut self, criteria: Criteria) -> Self {
		self.criteria = criteria;
		self
	}
}

impl<D: Datum> Builder<'_, D> {
	/// Builds the consumer instance and returns the receiver handle.
	pub fn build(self) -> Consumer<D> {
		worker::ReceiveWorker::<D>::spawn(
			&self.streams.local,
			&self.streams.discovery,
			&self.streams.config,
			self.criteria,
		)
	}
}
