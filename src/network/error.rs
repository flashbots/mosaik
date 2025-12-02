use crate::{discovery, groups, streams};

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("Missing network ID")]
	MissingNetworkId,

	#[error("Bind error: {0}")]
	Bind(#[from] iroh::endpoint::BindError),

	#[error("Discovery config error: {0}")]
	DiscoveryConfig(#[from] discovery::ConfigBuilderError),

	#[error("Streams config error: {0}")]
	StreamsConfig(#[from] streams::ConfigBuilderError),

	#[error("Groups config error: {0}")]
	GroupsConfig(#[from] groups::ConfigBuilderError),
}
