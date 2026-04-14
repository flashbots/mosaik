use {
	crate::{
		PeerId,
		primitives::{IntoIterOrSingle, Tag},
	},
	core::time::Duration,
	derive_builder::Builder,
	iroh::EndpointAddr,
	serde::{Deserialize, Serialize},
};

/// Configuration options for the discovery subsystem.
#[derive(Debug, Clone, Builder, Serialize, Deserialize, PartialEq)]
#[builder(pattern = "owned", setter(prefix = "with"), derive(Debug, Clone))]
#[builder_struct_attr(doc(hidden))]
pub struct Config {
	/// The maximum number of past events to retain in the event backlog in
	/// [`super::Discovery::events`] watchers.
	#[builder(default = "100")]
	pub events_backlog: usize,

	/// A list of bootstrap peers to connect to on startup.
	#[builder(default = "Vec::new()", setter(custom))]
	pub bootstrap_peers: Vec<EndpointAddr>,

	/// A list of tags to advertise in the local peer entry on startup.
	#[builder(default = "Vec::new()", setter(custom))]
	pub tags: Vec<Tag>,

	/// The duration after which stale peer entries are purged from the
	/// discovery catalog if no announcements are received from them.
	#[builder(default = "Duration::from_secs(300)")]
	pub purge_after: Duration,

	/// The maximum allowed time drift for peer entries.
	/// Entries with a timestamp outside this drift are considered invalid.
	#[builder(default = "Duration::from_secs(50)")]
	pub max_time_drift: Duration,

	/// The interval at which to announce our presence to the network.
	#[builder(default = "Duration::from_secs(45)")]
	pub announce_interval: Duration,

	/// The maximum jitter factor to apply to the announce interval.
	#[builder(default = "0.5")]
	pub announce_jitter: f32,

	/// Disables the DHT auto bootstrap mechanism.
	#[builder(default = "false")]
	pub no_auto_bootstrap: bool,

	/// The interval at which to ping cataloged peers to measure RTT.
	#[builder(default = "Duration::from_secs(30)")]
	pub rtt_probe_interval: Duration,

	/// Maximum acceptable RTT for peers. When set, peers whose smoothed
	/// RTT exceeds this threshold may be filtered by higher-level
	/// subsystems (streams, groups). Peers with no RTT data are not
	/// filtered (optimistic admission).
	#[builder(default = "None", setter(custom))]
	pub max_rtt: Option<Duration>,

	/// The duration the announcement protocol will wait for the graceful
	/// departure gossip message to propagate before shutting down.
	#[builder(setter(skip), default = "Duration::from_millis(500)")]
	pub graceful_departure_window: Duration,
}

impl Config {
	/// Creates a new config builder with default values.
	pub fn builder() -> ConfigBuilder {
		ConfigBuilder::default()
	}

	/// Returns the list of bootstrap peer IDs without their addresses.
	pub fn bootstrap_peers_ids(&self) -> Vec<PeerId> {
		self.bootstrap_peers.iter().map(|addr| addr.id).collect()
	}
}

impl ConfigBuilder {
	/// Adds bootstrap peer(s) to the discovery configuration.
	#[must_use]
	pub fn with_bootstrap<V>(
		mut self,
		peers: impl IntoIterOrSingle<EndpointAddr, V>,
	) -> Self {
		if let Some(existing) = &mut self.bootstrap_peers {
			existing.extend(peers.iterator());
		} else {
			self.bootstrap_peers = Some(peers.iterator().into_iter().collect());
		}
		self
	}

	/// Adds tag(s) to advertise in the local peer entry.
	#[must_use]
	pub fn with_tags<V>(mut self, tags: impl IntoIterOrSingle<Tag, V>) -> Self {
		let tags: Vec<Tag> = tags.iterator().into_iter().collect();
		if let Some(existing) = &mut self.tags {
			existing.extend(tags);
		} else {
			self.tags = Some(tags);
		}
		self
	}

	/// Disables the DHT auto bootstrap mechanism.
	///
	/// The node will need to be provided with a bootstrap peer manually by the
	/// API user.
	#[must_use]
	pub const fn no_auto_bootstrap(mut self) -> Self {
		self.no_auto_bootstrap = Some(true);
		self
	}

	/// Sets the maximum acceptable RTT for discovered peers.
	///
	/// When set, peers whose smoothed RTT exceeds this threshold may
	/// be filtered by higher-level subsystems. Peers with no RTT data
	/// are not filtered (optimistic admission — new peers are given a
	/// chance to establish direct paths before being evaluated).
	#[must_use]
	pub const fn with_max_rtt(mut self, max: Duration) -> Self {
		self.max_rtt = Some(Some(max));
		self
	}
}

#[doc(hidden)]
pub trait IntoConfig {
	fn into_config(self) -> Result<Config, ConfigBuilderError>;
}

impl IntoConfig for Config {
	fn into_config(self) -> Result<Config, ConfigBuilderError> {
		Ok(self)
	}
}

impl IntoConfig for ConfigBuilder {
	fn into_config(self) -> Result<Config, ConfigBuilderError> {
		self.build()
	}
}
