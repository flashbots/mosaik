use {
	core::{
		pin::Pin,
		task::{Context, Poll, Waker},
		time::Duration,
	},
	derive_more::Debug,
	futures::TryFutureExt,
	std::{process::Output, sync::OnceLock},
	tokio::time::Timeout,
};

pub fn poll_once<T, F>(f: &mut F) -> Poll<T>
where
	F: Future<Output = T> + Unpin,
{
	let waker = Waker::noop();
	let mut context = Context::from_waker(waker);
	Pin::new(f).poll(&mut context)
}
