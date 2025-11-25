use {
	super::{Key, Keyed},
	crate::prelude::{Accumulated, Datum},
	futures::Sink,
};

pub trait ProducerExt<D: Datum>: Sized {
	fn keyed_by<K: Key, F: Fn(&D) -> K + 'static>(
		self,
		_: F,
	) -> Keyed<Self, D, K>;

	fn accumulate<Acc, F>(self, fold_fn: F) -> Accumulated<Self, D, Acc, F>
	where
		Acc: Default,
		F: FnMut(&mut Acc, &D) + Unpin,
		Self: Sink<D>,
	{
		Accumulated::producer(self, fold_fn)
	}
}

impl<T: Sink<D>, D: Datum> ProducerExt<D> for T {
	fn keyed_by<K: Key, F: Fn(&D) -> K + 'static>(
		self,
		extractor: F,
	) -> Keyed<Self, D, K> {
		Keyed::<T, D, K>::producer(self, extractor)
	}
}
