pub use backoff;
use {
	backoff::{ExponentialBackoff, backoff::Backoff},
	core::fmt::Debug,
	derive_builder::Builder,
};

/// Configuration options for the streams subsystem.
#[derive(Builder)]
#[builder(pattern = "owned", setter(prefix = "with"))]
#[builder_struct_attr(doc(hidden))]
pub struct Config {
	/// The size of the producer buffer.
	#[builder(default = "1024")]
	pub producer_buffer_size: usize,

	/// The backoff policy for retrying stream connections.
	#[builder(
		setter(custom),
		default = "Box::new(|| Box::new(ExponentialBackoff::default()))"
	)]
	pub backoff: BackoffFactory,
}

impl Config {
	/// Creates a new config builder with default values.
	pub fn builder() -> ConfigBuilder {
		ConfigBuilder::default()
	}
}

impl ConfigBuilder {
	/// Sets a backoff policy for stream connection retries.
	#[must_use]
	pub fn with_backoff<B: Backoff + Clone + Send + Sync + 'static>(
		mut self,
		backoff: B,
	) -> Self {
		let backoff = backoff;
		self.backoff = Some(Box::new(move || Box::new(backoff.clone())));
		self
	}
}

impl core::fmt::Debug for Config {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		f.debug_struct("Config")
			.field("producer_buffer_size", &self.producer_buffer_size)
			.field("backoff", &"<backoff factory>")
			.finish()
	}
}

impl core::fmt::Debug for ConfigBuilder {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		f.debug_struct("Config")
			.field("producer_buffer_size", &self.producer_buffer_size)
			.field("backoff", &"<backoff factory>")
			.finish()
	}
}

pub type BackoffFactory = Box<
	dyn Fn() -> Box<dyn Backoff + Send + Sync + 'static> + Send + Sync + 'static,
>;
