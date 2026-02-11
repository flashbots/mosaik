use core::pin::Pin;

/// A boxed future that is `Send`, `Sync`, and `'static`.
pub type BoxPinFut<T> =
	Pin<Box<dyn Future<Output = T> + Send + Sync + 'static>>;

pub trait InternalFutureExt: Future {
	fn pin(self) -> BoxPinFut<Self::Output>
	where
		Self: Sized + Send + Sync + 'static,
	{
		Box::pin(self)
	}
}

impl<F: Future> InternalFutureExt for F {}
