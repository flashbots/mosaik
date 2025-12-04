use {std::io, tokio_util::sync::CancellationToken};

/// Runs a cancellable future that can be aborted via the provided cancellation
/// token.
///
/// If the future is cancelled, an `io::Error` with kind `Interrupted` is
/// returned.
///
/// If the future completes with an error, it is converted to an
/// `std::io::Error` via `std::io::Error::other`.
pub async fn cancellable<F, T, E>(
	cancel: &CancellationToken,
	fut: F,
) -> Result<T, io::Error>
where
	E: core::error::Error + Send + Sync + 'static,
	F: Future<Output = Result<T, E>> + Send + Sync,
{
	cancel
		.run_until_cancelled(fut)
		.await
		.ok_or_else(|| {
			io::Error::new(io::ErrorKind::Interrupted, "connection attempt cancelled")
		})?
		.map_err(io::Error::other)
}
