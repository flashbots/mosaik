use {
	crate::prelude::Datum,
	core::{
		marker::PhantomData,
		ops::{Deref, DerefMut},
		pin::Pin,
		task::{Context, Poll},
	},
	futures::{Sink, Stream},
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

impl<C, D: Datum, Acc, F> Accumulated<C, D, Acc, F>
where
	F: Fn(&mut Acc, D) + Unpin,
{
	pub fn accumulate(&mut self, datum: D) {
		(self.fold_fn)(&mut self.state, datum);
	}
}

impl<C, D: Datum, Acc, F> Accumulated<C, D, Acc, F>
where
	F: Fn(&mut Acc, D) + Unpin,
	C: Stream<Item = D> + Unpin,
{
	pub fn consumer(underlying: C, fold_fn: F) -> Self
	where
		Acc: Default,
	{
		Self {
			fold_fn,
			state: Acc::default(),
			underlying,
			_p: PhantomData,
		}
	}

	pub fn consumer_with_state(underlying: C, initial: Acc, fold_fn: F) -> Self {
		Self {
			fold_fn,
			state: initial,
			underlying,
			_p: PhantomData,
		}
	}
}

impl<C, D: Datum, Acc, F> Accumulated<C, D, Acc, F>
where
	F: Fn(&mut Acc, D) + Unpin,
	C: Sink<D> + Unpin,
{
	pub fn producer(underlying: C, fold_fn: F) -> Self
	where
		Acc: Default,
	{
		Self {
			fold_fn,
			state: Acc::default(),
			underlying,
			_p: PhantomData,
		}
	}

	pub fn producer_with_state(underlying: C, initial: Acc, fold_fn: F) -> Self {
		Self {
			fold_fn,
			state: initial,
			underlying,
			_p: PhantomData,
		}
	}
}

impl<C, D: Datum, Acc, F> Deref for Accumulated<C, D, Acc, F> {
	type Target = C;

	fn deref(&self) -> &Self::Target {
		&self.underlying
	}
}

impl<C, D: Datum, Acc, F> DerefMut for Accumulated<C, D, Acc, F> {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.underlying
	}
}

impl<C, D, Acc, F> Sink<D> for Accumulated<C, D, Acc, F>
where
	D: Datum + Clone,
	F: Fn(&mut Acc, D) + Unpin,
	Acc: Unpin,
	C: Sink<D> + Unpin,
{
	type Error = <C as Sink<D>>::Error;

	fn poll_ready(
		self: std::pin::Pin<&mut Self>,
		cx: &mut std::task::Context<'_>,
	) -> std::task::Poll<Result<(), Self::Error>> {
		let this = self.get_mut();
		C::poll_ready(Pin::new(&mut this.underlying), cx)
	}

	fn start_send(
		self: std::pin::Pin<&mut Self>,
		item: D,
	) -> Result<(), Self::Error> {
		let this = self.get_mut();
		(this.fold_fn)(&mut this.state, item.clone());
		C::start_send(Pin::new(&mut this.underlying), item)
	}

	fn poll_flush(
		self: std::pin::Pin<&mut Self>,
		cx: &mut std::task::Context<'_>,
	) -> std::task::Poll<Result<(), Self::Error>> {
		let this = self.get_mut();
		C::poll_flush(Pin::new(&mut this.underlying), cx)
	}

	fn poll_close(
		self: std::pin::Pin<&mut Self>,
		cx: &mut std::task::Context<'_>,
	) -> std::task::Poll<Result<(), Self::Error>> {
		let this = self.get_mut();
		C::poll_close(Pin::new(&mut this.underlying), cx)
	}
}

impl<C, D, Acc, F> Stream for Accumulated<C, D, Acc, F>
where
	D: Datum + Clone,
	C: Stream<Item = D> + Unpin,
	F: Fn(&mut Acc, D) + Unpin,
	Acc: Unpin,
{
	type Item = D;

	fn poll_next(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Self::Item>> {
		let this = self.get_mut();
		tracing::info!("Polling next on Accumulated stream");
		match Pin::new(&mut this.underlying).poll_next(cx) {
			Poll::Ready(Some(datum)) => {
				(this.fold_fn)(&mut this.state, datum.clone());
				Poll::Ready(Some(datum))
			}
			Poll::Ready(None) => Poll::Ready(None),
			Poll::Pending => Poll::Pending,
		}
	}
}
