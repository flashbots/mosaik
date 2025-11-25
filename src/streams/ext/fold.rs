use {
	crate::prelude::{Datum, Key, Keyed, KeyedDatum, Producer},
	core::{marker::PhantomData, pin::Pin},
	futures::Sink,
};

pub struct Accumulated<C, D: Datum, Acc, F> {
	fold_fn: F,
	state: Acc,
	underlying: C,
	_p: PhantomData<D>,
}

impl<C, D: Datum, Acc, F> Accumulated<C, D, Acc, F> {
	pub const fn state(&self) -> &Acc {
		&self.state
	}

	pub const fn state_mut(&mut self) -> &mut Acc {
		&mut self.state
	}
}

impl<D, K, Acc, F> Accumulated<Keyed<Producer<D>, D, K>, D, Acc, F>
where
	D: Datum,
	K: Key,
	F: Fn(&mut Acc, KeyedDatum<D, K>) + Unpin,
{
	pub fn new(producer: Keyed<Producer<D>, D, K>, fold_fn: F) -> Self
	where
		Acc: Default + Unpin,
	{
		Self {
			fold_fn,
			state: Acc::default(),
			underlying: producer,
			_p: PhantomData,
		}
	}

	pub fn new_with_state(
		producer: Keyed<Producer<D>, D, K>,
		initial: Acc,
		fold_fn: F,
	) -> Self {
		Self {
			fold_fn,
			state: initial,
			underlying: producer,
			_p: PhantomData,
		}
	}
}

impl<C, D, Acc, F> AsRef<C> for Accumulated<C, D, Acc, F>
where
	D: Datum,
{
	fn as_ref(&self) -> &C {
		&self.underlying
	}
}

impl<C, D, Acc, F> AsMut<C> for Accumulated<C, D, Acc, F>
where
	D: Datum,
{
	fn as_mut(&mut self) -> &mut C {
		&mut self.underlying
	}
}

impl<D, K, Acc, F> Sink<D> for Accumulated<Keyed<Producer<D>, D, K>, D, Acc, F>
where
	D: Datum + Clone,
	K: Key,
	F: Fn(&mut Acc, KeyedDatum<D, K>) + Unpin,
	Acc: Unpin,
{
	type Error = <Producer<D> as Sink<D>>::Error;

	fn poll_ready(
		self: std::pin::Pin<&mut Self>,
		cx: &mut std::task::Context<'_>,
	) -> std::task::Poll<Result<(), Self::Error>> {
		let this = self.get_mut();
		Producer::poll_ready(Pin::new(&mut this.underlying), cx)
	}

	fn start_send(
		self: std::pin::Pin<&mut Self>,
		item: D,
	) -> Result<(), Self::Error> {
		let this = self.get_mut();
		let key = this.underlying.key_of(&item);
		let keyed_datum = KeyedDatum(key, item.clone());
		(this.fold_fn)(&mut this.state, keyed_datum);
		Producer::start_send(Pin::new(&mut this.underlying), item)
	}

	fn poll_flush(
		self: std::pin::Pin<&mut Self>,
		cx: &mut std::task::Context<'_>,
	) -> std::task::Poll<Result<(), Self::Error>> {
		let this = self.get_mut();
		Producer::poll_flush(Pin::new(&mut this.underlying), cx)
	}

	fn poll_close(
		self: std::pin::Pin<&mut Self>,
		cx: &mut std::task::Context<'_>,
	) -> std::task::Poll<Result<(), Self::Error>> {
		let this = self.get_mut();
		Producer::poll_close(Pin::new(&mut this.underlying), cx)
	}
}
