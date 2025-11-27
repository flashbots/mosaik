use {
	super::{Key, Keyed},
	crate::prelude::Accumulated,
	futures::Sink,
};

pub trait ProducerExt<D>: Sized + Sink<D> {
	fn keyed_by<K: Key, F: Fn(&D) -> K + 'static>(
		self,
		extractor: F,
	) -> Keyed<Self, D, K> {
		Keyed::<Self, D, K>::producer(self, extractor)
	}

	fn accumulate<Acc, F>(self, fold_fn: F) -> Accumulated<Self, Acc, F>
	where
		Acc: Default,
		F: FnMut(&mut Acc, &D) + Unpin,
		Self: Sink<D>,
	{
		Accumulated::producer(self, fold_fn)
	}
}

impl<T: Sink<D>, D> ProducerExt<D> for T {}
