mod discovery;
mod groups;
mod id;
mod local;
mod network;
mod streams;

pub mod prelude {
	pub use super::{
		discovery::{
			Catalog,
			Error as DiscoveryError,
			Event as DiscoveryEvent,
			Events as DiscoveryEvents,
			PeerId,
			PeerInfo,
			SignedPeerInfo,
		},
		id::NetworkId,
		network::{Error as NetworkError, Network},
		streams::{Criteria, Datum, Error as StreamsError, Producer},
	};
}

#[cfg(feature = "test-utils")]
pub mod test_utils;
