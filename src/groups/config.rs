use {
	crate::primitives::BackoffFactory,
	backoff::{ExponentialBackoffBuilder, backoff::Backoff},
	core::time::Duration,
	derive_builder::Builder,
	std::sync::Arc,
};

/// Configuration options for the streams subsystem.
#[derive(Builder)]
#[builder(pattern = "owned", setter(prefix = "with"))]
#[builder_struct_attr(doc(hidden))]
pub struct Config {
	/// The timeout duration for completing the handshake when establishing
	/// a new bond connection to a remote peer in the group.
	#[builder(default = "Duration::from_secs(3)")]
	pub handshake_timeout: Duration,

	/// The interval at which heartbeat messages are sent over established
	/// bonds to peers in the group to ensure liveness of the connection.
	#[builder(default = "Duration::from_secs(2)")]
	pub heartbeat_interval: Duration,

	/// The maximum jitter to apply to the heartbeat interval to avoid
	/// an avalanche of heartbeats being sent at the same time.
	///
	/// heartbeats are sent at intervals of
	/// `heartbeat_interval - rand(0, heartbeat_jitter)`.
	#[builder(default = "Duration::from_secs(1)")]
	pub heartbeat_jitter: Duration,

	/// The maximum number of consecutive missed heartbeats before considering
	/// the bond connection to be dead and closing it.
	#[builder(default = "5")]
	pub max_missed_heartbeats: usize,

	/// The backoff policy for retrying establishing persistent connections with
	/// peers in a group on recoverable failures. This is the default policy
	/// used unless overridden per-connection. The default is an exponential
	/// backoff with a maximum elapsed time of 5 minutes.
	#[builder(
		setter(custom),
		default = "Arc::new(|| Box::new(ExponentialBackoffBuilder::default() \
		           .with_max_elapsed_time(Some(Duration::from_secs(300))) \
		           .build()))"
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
		self.backoff = Some(Arc::new(move || Box::new(backoff.clone())));
		self
	}
}

impl core::fmt::Debug for Config {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		f.debug_struct("Config")
			.field("handshake_timeout", &self.handshake_timeout)
			.field("heartbeat_interval", &self.heartbeat_interval)
			.field("heartbeat_jitter", &self.heartbeat_jitter)
			.field("max_missed_heartbeats", &self.max_missed_heartbeats)
			.field("backoff", &"<backoff factory>")
			.finish()
	}
}

impl core::fmt::Debug for ConfigBuilder {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		f.debug_struct("Config")
			.field("handshake_timeout", &self.handshake_timeout)
			.field("heartbeat_interval", &self.heartbeat_interval)
			.field("heartbeat_jitter", &self.heartbeat_jitter)
			.field("max_missed_heartbeats", &self.max_missed_heartbeats)
			.field("backoff", &"<backoff factory>")
			.finish()
	}
}
