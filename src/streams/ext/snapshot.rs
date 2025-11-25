use {
	crate::prelude::{Accumulated, Datum, Producer, ProducerExt},
	core::{hash::Hash, marker::PhantomData},
	futures::Stream,
};

pub struct SnapshotMapProducer<S, K, D>
where
	K: Eq + Ord + Hash + Clone,
	D: Datum + Clone,
	S: Stream<Item = D>,
{
	accumulator: Accumulated<
		S,
		D,
		im::OrdMap<K, D>,
		Box<dyn Fn(&mut im::OrdMap<K, D>, &D) + Unpin + 'static>,
	>,
}

impl<S, K, D> SnapshotMapProducer<S, K, D>
where
	K: Eq + Ord + Hash + Clone,
	D: Datum + Clone,
	S: Stream<Item = D>,
{
	pub fn new<I>(producer: Producer<D>, extract_fn: impl Fn(&D) -> I) -> Self
	where
		K: Eq + Ord + Hash + Clone,
		I: IntoIterator<Item = (K, D)>,
	{
		// let accumulator =
		// 	producer.accumulate(Box::new(|state: &mut im::OrdMap<K, D>, datum: &D| {
		// 		for (key, value) in extract_fn(datum) {
		// 			state.insert(key, value);
		// 		}
		// 	})
		// 		as Box<dyn Fn(&mut im::OrdMap<K, D>, &D) + Unpin + 'static>);

		// Self { accumulator }
		todo!()
	}
}

pub struct SnapshotMapConsumer<D: Datum>(PhantomData<D>);
