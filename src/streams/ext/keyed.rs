use {
	crate::prelude::{Consumer, Datum, Producer},
	core::{
		hash::Hash,
		ops::{Deref, DerefMut},
		pin::Pin,
		task::{Context, Poll},
	},
	futures::Stream,
};

pub trait Key: Eq + Ord + Hash {}
impl<T> Key for T where T: Eq + Ord + Hash {}

/// Represents a `Datum` that has been assigned a `Key`.
///
/// Notes:
/// - When stream datums are keyed they gain more properties that enable
///   joining, deduplication, ranges, etc.
#[derive(Debug)]
pub struct KeyedDatum<D: Datum, K: Key>(pub K, pub D);

impl<D, K> Clone for KeyedDatum<D, K>
where
	D: Datum + Clone,
	K: Key + Clone,
{
	fn clone(&self) -> Self {
		Self(self.0.clone(), self.1.clone())
	}
}

impl<D: Datum, K: Key> KeyedDatum<D, K> {
	pub const fn key(&self) -> &K {
		&self.0
	}

	pub const fn item(&self) -> &D {
		&self.1
	}

	pub fn into_value(self) -> D {
		self.1
	}

	pub fn into_key(self) -> K {
		self.0
	}

	pub fn split(self) -> (K, D) {
		let KeyedDatum(k, d) = self;
		(k, d)
	}
}

impl<D: Datum, K: Key> Ord for KeyedDatum<D, K> {
	fn cmp(&self, other: &Self) -> std::cmp::Ordering {
		self.key().cmp(other.key())
	}
}

impl<D: Datum, K: Key> PartialOrd for KeyedDatum<D, K> {
	fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
		Some(self.cmp(other))
	}
}

impl<D: Datum, K: Key> PartialEq for KeyedDatum<D, K> {
	fn eq(&self, other: &Self) -> bool {
		self.key() == other.key()
	}
}
impl<D: Datum, K: Key> Eq for KeyedDatum<D, K> {}

impl<D: Datum, K: Key> Deref for KeyedDatum<D, K> {
	type Target = D;

	fn deref(&self) -> &Self::Target {
		&self.1
	}
}

impl<D: Datum, K: Key> From<KeyedDatum<D, K>> for (K, D) {
	fn from(kd: KeyedDatum<D, K>) -> Self {
		kd.split()
	}
}

pub struct Keyed<C, D: Datum, K: Key> {
	underlying: C,
	key_fn: Box<dyn Fn(&D) -> K + 'static>,
}

impl<C, D: Datum, K: Key> Keyed<C, D, K> {
	pub fn key_of(&self, datum: &D) -> K {
		(self.key_fn)(datum)
	}
}

impl<D: Datum, K: Key> Keyed<Consumer<D>, D, K> {
	pub fn new(
		consumer: Consumer<D>,
		extractor: impl Fn(&D) -> K + 'static,
	) -> Self {
		Self {
			underlying: consumer,
			key_fn: Box::new(extractor),
		}
	}
}

impl<D: Datum, K: Key> Stream for Keyed<Consumer<D>, D, K> {
	type Item = KeyedDatum<D, K>;

	fn poll_next(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Self::Item>> {
		let this = self.get_mut();
		match Pin::new(&mut this.underlying).poll_next(cx) {
			Poll::Ready(Some(datum)) => {
				let key = (this.key_fn)(&datum);
				Poll::Ready(Some(KeyedDatum(key, datum)))
			}
			Poll::Ready(None) => Poll::Ready(None),
			Poll::Pending => Poll::Pending,
		}
	}
}

impl<C, D: Datum, K: Key> Deref for Keyed<C, D, K> {
	type Target = C;

	fn deref(&self) -> &Self::Target {
		&self.underlying
	}
}

impl<C, D: Datum, K: Key> DerefMut for Keyed<C, D, K> {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.underlying
	}
}

impl<D: Datum, K: Key> Keyed<Producer<D>, D, K> {
	pub fn new(
		consumer: Producer<D>,
		extractor: impl Fn(&D) -> K + 'static,
	) -> Self {
		Self {
			underlying: consumer,
			key_fn: Box::new(extractor),
		}
	}
}
