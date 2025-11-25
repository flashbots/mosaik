use {
	super::{Key, Keyed},
	crate::prelude::{Consumer, Datum},
};

pub trait ConsumerExt<D: Datum>: Sized {
	fn keyed_by<K: Key, F: Fn(&D) -> K + 'static>(
		self,
		_: F,
	) -> Keyed<Self, D, K>;
}

impl<D: Datum> ConsumerExt<D> for Consumer<D> {
	fn keyed_by<K: Key, F: Fn(&D) -> K + 'static>(
		self,
		extractor: F,
	) -> Keyed<Self, D, K> {
		Keyed::<Consumer<D>, D, K>::new(self, extractor)
	}
}
