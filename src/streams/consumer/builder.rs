use super::{super::Streams, Consumer, Datum};

pub struct Builder<'s, D: Datum> {
	streams: &'s Streams,
	_marker: std::marker::PhantomData<D>,
}

impl<'s, D: Datum> Builder<'s, D> {
	pub(crate) fn new(streams: &'s Streams) -> Self {
		Self {
			streams,
			_marker: std::marker::PhantomData,
		}
	}
}

impl<D: Datum> Builder<'_, D> {
	pub fn build(&self) -> Consumer<D> {
		Consumer {
			_marker: std::marker::PhantomData,
		}
	}
}
