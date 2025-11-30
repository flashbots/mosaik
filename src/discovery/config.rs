use {
	crate::{PeerId, Tag},
	derive_builder::Builder,
	serde::{Deserialize, Serialize},
};

/// Configuration options for the discovery subsystem.
#[derive(Debug, Clone, Builder, Serialize, Deserialize, PartialEq, Hash)]
#[builder(pattern = "owned", setter(prefix = "with"))]
pub struct Config {
	/// The maximum number of past events to retain in the event backlog in
	/// [`Discovery::events`] watchers.
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
	/// Adds a bootstrap peer to the configuration.
	#[must_use]
	pub fn with_bootstrap_peer(mut self, peer: impl Into<PeerId>) -> Self {
		if let Some(peers) = &mut self.bootstrap_peers {
			peers.push(peer.into());
		} else {
			self.bootstrap_peers = Some(vec![peer.into()]);
		}
		self
	}

	/// Adds a list of bootstrap peers to the configuration.
	#[must_use]
	pub fn with_bootstrap_peers(
		mut self,
		peers: impl IntoIterator<Item = impl Into<PeerId>>,
	) -> Self {
		if let Some(existing) = &mut self.bootstrap_peers {
			existing.extend(peers.into_iter().map(|p| p.into()));
		} else {
			self.bootstrap_peers =
				Some(peers.into_iter().map(|p| p.into()).collect());
		}
		self
	}
}
