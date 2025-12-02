use super::{
	super::{Datum, Streams},
	Producer,
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
		self.streams.sinks.open_or_create::<D>().await.sender()
	}
}
