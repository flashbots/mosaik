use {
	super::{Key, Keyed},
	crate::prelude::{Accumulated, Datum},
	futures::Stream,
};

pub trait ConsumerExt<D: Datum>: Sized {
	fn keyed_by<K: Key, F: Fn(&D) -> K + 'static>(
		self,
		_: F,
	) -> Keyed<Self, D, K>;

	fn accumulate<Acc, F>(self, fold_fn: F) -> Accumulated<Self, D, Acc, F>
	where
		Acc: Default,
		F: FnMut(&mut Acc, &D) + Unpin,
		Self: Stream<Item = D> + Unpin,
	{
		Accumulated::consumer(self, fold_fn)
	}
}

impl<T: Stream<Item = D>, D: Datum> ConsumerExt<D> for T {
	fn keyed_by<K: Key, F: Fn(&D) -> K + 'static>(
		self,
		extractor: F,
	) -> Keyed<Self, D, K> {
		Keyed::<T, D, K>::consumer(self, extractor)
	}
}
