use {
	crate::{
		prelude::Datum,
		streams::ext::{Key, KeyedDatum},
	},
	core::{
		pin::Pin,
		task::{Context, Poll},
	},
	futures::{Stream, StreamExt},
	std::collections::HashMap,
};

pub struct Join<S1, S2, L, R, K>
where
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
	L: Unpin,
	R: Unpin,
	S1: Stream<Item = KeyedDatum<L, K>> + Unpin,
	S2: Stream<Item = KeyedDatum<R, K>> + Unpin,
	K: Key + core::fmt::Debug + Clone + Unpin,
{
	type Item = KeyedDatum<(L, R), K>;

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
				return Some(KeyedDatum(k, (left, right)));
			}
			None
		};

		if let Some(item) = try_emit(this) {
			return Poll::Ready(Some(item));
		}

		// poll left
		match this.left.poll_next_unpin(cx) {
			Poll::Ready(Some(KeyedDatum(k, l))) => {
				tracing::info!("Join: received left item with key {k:?}");
				this.buf_left.insert(k, l);
			}
			Poll::Ready(None) | Poll::Pending => {}
		}

		if let Some(item) = try_emit(this) {
			return Poll::Ready(Some(item));
		}

		// poll right
		match this.right.poll_next_unpin(cx) {
			Poll::Ready(Some(KeyedDatum(k, r))) => {
				tracing::info!("Join: received right item with key {k:?}");
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
