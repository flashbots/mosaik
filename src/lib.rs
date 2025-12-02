mod discovery;
#[allow(dead_code)]
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
			network::{Error as NetworkError, Network, NetworkBuilder},
			streams::{
				Consumer,
				ConsumerError,
				ConsumerStatus,
				Criteria,
				Datum,
				Error as StreamsError,
				Producer,
				StreamId,
			},
		},
		futures::{SinkExt, StreamExt},
	};
}

pub use iroh::{EndpointAddr, EndpointId, PublicKey, SecretKey};

#[cfg(feature = "test-utils")]
pub mod test_utils;
