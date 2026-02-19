use core::pin::Pin;

/// A boxed future that is `Send` and `'static`.
pub type BoxPinFut<T: core::fmt::Debug> =
	Pin<Box<dyn Future<Output = T> + Send + 'static>>;

pub trait InternalFutureExt: Future {
	fn pin(self) -> BoxPinFut<Self::Output>
	where
		Self: Sized + Send + 'static,
	{
		Box::pin(self)
	}
}

impl<F: Future> InternalFutureExt for F {}
