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

/// Applies a time factor to durations for testing purposes.
/// this for example when you are developing on an airplane wifi and
/// have very high latencies.
fn time_factor() -> f32 {
	static MULTIPLIER: OnceLock<f32> = OnceLock::new();
	*MULTIPLIER.get_or_init(|| {
		std::env::var("TIME_FACTOR")
			.ok()
			.and_then(|s| s.parse().ok())
			.unwrap_or(1.0)
	})
}

pub fn secs(count: u64) -> Duration {
	let count = (count as f32 * time_factor()) * 1000.0;
	#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
	Duration::from_millis(count as u64)
}

pub fn millis(count: u64) -> Duration {
	#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
	let count = (count as f32 * time_factor()) as u64;
	Duration::from_millis(count)
}

#[derive(Debug, Clone, Copy)]
pub struct TimeoutElapsed(pub Duration);

pub fn timeout_s<F>(
	count: u64,
	future: F,
) -> impl Future<Output = Result<F::Output, TimeoutElapsed>>
where
	F: IntoFuture,
{
	let duration = secs(count);
	tokio::time::timeout(duration, future)
		.map_err(move |_| TimeoutElapsed(duration))
}

pub fn timeout_ms<F>(
	count: u64,
	future: F,
) -> impl Future<Output = Result<F::Output, TimeoutElapsed>>
where
	F: IntoFuture,
{
	let duration = millis(count);
	tokio::time::timeout(duration, future)
		.map_err(move |_| TimeoutElapsed(duration))
}
