mod discovery;
mod groups;
mod id;
mod network;
mod streams;

pub use {
	discovery::{
		Catalog,
		Config as DiscoveryConfig,
		ConfigBuilder as DiscoveryConfigBuilder,
		Discovery,
		Error as DiscoveryError,
		Event as DiscoveryEvent,
		PeerEntry,
		SignedPeerEntry,
	},
	groups::*,
	id::*,
	iroh::{
		Endpoint,
		EndpointAddr,
		EndpointId,
		PublicKey,
		SecretKey,
		Signature,
		TransportAddr,
	},
	network::*,
	streams::{
		Config as StreamsConfig,
		ConfigBuilder as StreamsConfigBuilder,
		StreamId,
		Streams,
	},
};

#[cfg(feature = "test-utils")]
pub mod test_utils;
