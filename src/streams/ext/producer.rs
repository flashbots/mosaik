use {
	super::{Key, Keyed},
	crate::prelude::{Datum, Producer},
};

pub trait ProducerExt<D: Datum>: Sized {
	fn keyed_by<K: Key, F: Fn(&D) -> K + 'static>(
		self,
		_: F,
	) -> Keyed<Self, D, K>;
}

impl<D: Datum> ProducerExt<D> for Producer<D> {
	fn keyed_by<K: Key, F: Fn(&D) -> K + 'static>(
		self,
		extractor: F,
	) -> Keyed<Self, D, K> {
		Keyed::<Producer<D>, D, K>::new(self, extractor)
	}
}
