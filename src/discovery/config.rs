use {
	crate::{
		network::PeerId,
		primitives::{IntoIterOrSingle, Tag},
	},
	derive_builder::Builder,
	serde::{Deserialize, Serialize},
};

/// Configuration options for the discovery subsystem.
#[derive(Debug, Clone, Builder, Serialize, Deserialize, PartialEq, Hash)]
#[builder(pattern = "owned", setter(prefix = "with"), derive(Debug, Clone))]
#[builder_struct_attr(doc(hidden))]
pub struct Config {
	/// The maximum number of past events to retain in the event backlog in
	/// [`super::Discovery::events`] watchers.
	#[builder(default = "100")]
	pub events_backlog: usize,

	/// A list of bootstrap peers to connect to on startup.
	#[builder(default = "Vec::new()", setter(custom))]
	pub bootstrap_peers: Vec<PeerId>,

	/// A list of tags to advertise in the local peer entry on startup.
	#[builder(default = "Vec::new()", setter(custom))]
	pub tags: Vec<Tag>,
}

impl Config {
	/// Creates a new config builder with default values.
	pub fn builder() -> ConfigBuilder {
		ConfigBuilder::default()
	}
}

impl ConfigBuilder {
	/// Adds bootstrap peer(s) to the discovery configuration.
	#[must_use]
	pub fn with_bootstrap<V>(
		mut self,
		peers: impl IntoIterOrSingle<PeerId, V>,
	) -> Self {
		let peers: Vec<PeerId> = peers.iterator().into_iter().collect();
		if let Some(existing) = &mut self.bootstrap_peers {
			existing.extend(peers);
		} else {
			self.bootstrap_peers = Some(peers);
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
