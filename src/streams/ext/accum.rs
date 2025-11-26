use {
	core::{
		ops::{Deref, DerefMut},
		pin::Pin,
		task::{Context, Poll},
	},
	futures::{Sink, Stream},
};

pub struct Accumulated<C, Acc, F> {
	fold_fn: F,
	state: Acc,
	underlying: C,
}

impl<C, Acc, F> Accumulated<C, Acc, F> {
	pub const fn state(&self) -> &Acc {
		&self.state
	}

	pub const fn state_mut(&mut self) -> &mut Acc {
		&mut self.state
	}
}

impl<C, D, Acc, F> Accumulated<C, Acc, F>
where
	F: FnMut(&mut Acc, &D) + Unpin,
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
		}
	}

	pub fn consumer_with_state(underlying: C, initial: Acc, fold_fn: F) -> Self {
		Self {
			fold_fn,
			state: initial,
			underlying,
		}
	}
}

impl<C, Acc, F> Accumulated<C, Acc, F> {
	pub fn producer<D>(underlying: C, fold_fn: F) -> Self
	where
		Acc: Default,
		F: FnMut(&mut Acc, &D) + Unpin,
		C: Sink<D>,
	{
		Self {
			fold_fn,
			state: Acc::default(),
			underlying,
		}
	}

	pub fn producer_with_state(underlying: C, initial: Acc, fold_fn: F) -> Self {
		Self {
			fold_fn,
			state: initial,
			underlying,
		}
	}
}

impl<C, Acc, F> Deref for Accumulated<C, Acc, F> {
	type Target = C;

	fn deref(&self) -> &Self::Target {
		&self.underlying
	}
}

impl<C, Acc, F> DerefMut for Accumulated<C, Acc, F> {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.underlying
	}
}

impl<C, Acc, F> Unpin for Accumulated<C, Acc, F> {}

impl<C, D, Acc, F> Sink<D> for Accumulated<C, Acc, F>
where
	F: FnMut(&mut Acc, &D) + Unpin,
	Acc: Unpin,
	C: Sink<D> + Unpin,
{
	type Error = <C as Sink<D>>::Error;

	fn poll_ready(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Result<(), Self::Error>> {
		let this = self.get_mut();
		C::poll_ready(Pin::new(&mut this.underlying), cx)
	}

	fn start_send(self: Pin<&mut Self>, item: D) -> Result<(), Self::Error> {
		let this = self.get_mut();
		(this.fold_fn)(&mut this.state, &item);
		C::start_send(Pin::new(&mut this.underlying), item)
	}

	fn poll_flush(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Result<(), Self::Error>> {
		let this = self.get_mut();
		C::poll_flush(Pin::new(&mut this.underlying), cx)
	}

	fn poll_close(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Result<(), Self::Error>> {
		let this = self.get_mut();
		C::poll_close(Pin::new(&mut this.underlying), cx)
	}
}

impl<C, D, Acc, F> Stream for Accumulated<C, Acc, F>
where
	C: Stream<Item = D> + Unpin,
	F: FnMut(&mut Acc, &D) + Unpin,
	Acc: Unpin,
{
	type Item = D;

	fn poll_next(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Self::Item>> {
		let this = self.get_mut();
		match Pin::new(&mut this.underlying).poll_next(cx) {
			Poll::Ready(Some(datum)) => {
				(this.fold_fn)(&mut this.state, &datum);
				Poll::Ready(Some(datum))
			}
			Poll::Ready(None) => Poll::Ready(None),
			Poll::Pending => Poll::Pending,
		}
	}
}
