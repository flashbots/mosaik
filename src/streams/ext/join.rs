use {
	crate::{
		prelude::{Consumer, Datum},
		streams::ext::{Append, Key, Keyed, KeyedDatum},
	},
	core::{
		pin::Pin,
		task::{Context, Poll},
	},
	futures::Stream,
	std::collections::HashMap,
};

pub struct Join<S1, S2, L, R, K>
where
	L: Datum,
	R: Datum,
	K: Key,
	S1: Stream<Item = KeyedDatum<L, K>>,
	S2: Stream<Item = KeyedDatum<R, K>>,
{
	left: S1,
	right: S2,
	buf_left: HashMap<K, L>,
	buf_right: HashMap<K, R>,
}

impl<S1, S2, L, R, K> Join<S1, S2, L, R, K>
where
	L: Datum,
	R: Datum,
	K: Key,
	S1: Stream<Item = KeyedDatum<L, K>>,
	S2: Stream<Item = KeyedDatum<R, K>>,
{
	pub fn new(left: S1, right: S2) -> Self {
		Self {
			left,
			right,
			buf_left: HashMap::new(),
			buf_right: HashMap::new(),
		}
	}
}

impl<S1, S2, L, R, K> Stream for Join<S1, S2, L, R, K>
where
	S1: Stream<Item = KeyedDatum<L, K>> + Unpin,
	S2: Stream<Item = KeyedDatum<R, K>> + Unpin,
	L: Datum + Append<R> + Unpin,
	R: Datum + Unpin,
	<L as Append<R>>::Out: Datum + Unpin,
	K: Key + Clone + Unpin,
{
	type Item = KeyedDatum<<L as Append<R>>::Out, K>;

	fn poll_next(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Self::Item>> {
		let this = self.get_mut();

		// check if we already have a match pending
		let try_emit = |this: &mut Self| {
			if let Some(k) = this
				.buf_left
				.keys()
				.find(|k| this.buf_right.contains_key(*k))
				.cloned()
			{
				let left = this.buf_left.remove(&k).unwrap();
				let right = this.buf_right.remove(&k).unwrap();
				let joined = left.append(right);
				return Some(KeyedDatum(k, joined));
			}
			None
		};

		if let Some(item) = try_emit(this) {
			return Poll::Ready(Some(item));
		}

		// poll left
		match Pin::new(&mut this.left).poll_next(cx) {
			Poll::Ready(Some(KeyedDatum(k, l))) => {
				this.buf_left.insert(k, l);
			}
			Poll::Ready(None) | Poll::Pending => {}
		}

		if let Some(item) = try_emit(this) {
			return Poll::Ready(Some(item));
		}

		// poll right
		match Pin::new(&mut this.right).poll_next(cx) {
			Poll::Ready(Some(KeyedDatum(k, r))) => {
				this.buf_right.insert(k, r);
			}
			Poll::Ready(None) | Poll::Pending => {}
		}

		if let Some(item) = try_emit(this) {
			return Poll::Ready(Some(item));
		}

		Poll::Pending
	}
}
