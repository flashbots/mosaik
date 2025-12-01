mod network;
mod primitives;

pub mod discovery;
pub mod groups;
pub mod streams;

pub use {
	discovery::{
		AnnounceEvent,
		Catalog,
		Config as DiscoveryConfig,
		ConfigBuilder as DiscoveryConfigBuilder,
		Discovery,
		Error as DiscoveryError,
		Event as DiscoveryEvent,
		PeerEntry,
		PeerEntryVersion,
		SignedPeerEntry,
		SyncEvent,
	},
	groups::*,
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
	primitives::*,
	streams::{
		Config as StreamsConfig,
		ConfigBuilder as StreamsConfigBuilder,
		Consumer,
		Datum,
		Producer,
		StreamId,
		Streams,
	},
};

/// Used internally as a sentinel type for generic parameters.
#[doc(hidden)]
pub enum Variant<const U: usize = 0> {}

#[cfg(feature = "test-utils")]
pub mod test_utils;
