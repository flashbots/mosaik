use {
	crate::streams::{Criteria, Datum, StreamId},
	core::{
		pin::Pin,
		task::{Context, Poll},
	},
	futures::Stream,
};

pub struct Consumer<D: Datum> {
	stream_id: StreamId,
	criteria: Criteria,
	_marker: std::marker::PhantomData<D>,
}

impl<D: Datum> Consumer<D> {
	pub fn new(stream_id: StreamId, criteria: Criteria) -> Self {
		Self {
			stream_id,
			criteria,
			_marker: std::marker::PhantomData,
		}
	}

	pub const fn stream_id(&self) -> &StreamId {
		&self.stream_id
	}

	pub const fn criteria(&self) -> &Criteria {
		&self.criteria
	}
}

impl<D: Datum> Stream for Consumer<D> {
	type Item = D;

	fn poll_next(
		self: Pin<&mut Self>,
		_cx: &mut Context<'_>,
	) -> Poll<Option<Self::Item>> {
		todo!()
	}
}
