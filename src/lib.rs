mod discovery;
mod groups;
mod id;
mod local;
mod network;
mod rpc;
mod streams;

pub mod prelude {
	pub use {
		super::{
			discovery::{
				Catalog,
				Error as DiscoveryError,
				Event as DiscoveryEvent,
				Events as DiscoveryEvents,
				PeerId,
				PeerInfo,
				SignedPeerInfo,
			},
			groups::{Group, GroupDef, GroupHash, GroupKey, GroupState},
			id::NetworkId,
			network::{Error as NetworkError, Network},
			streams::{Criteria, Datum, Error as StreamsError, Producer},
		},
		futures::{SinkExt, StreamExt},
	};
}

#[cfg(feature = "test-utils")]
pub mod test_utils;
