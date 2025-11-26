mod datum;
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
			datum::{Criteria, Datum, StreamId},
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
			streams::{
				Consumer,
				ConsumerExt,
				Error as StreamsError,
				Producer,
				ProducerExt,
				*,
			},
		},
		futures::{SinkExt, StreamExt},
		iroh::{
			self,
			EndpointAddr,
			EndpointId,
			KeyParsingError,
			PublicKey,
			SecretKey,
			Signature,
			SignatureError,
			TransportAddr,
		},
		serde::{Deserialize, Serialize},
	};
}

#[cfg(feature = "test-utils")]
pub mod test_utils;
