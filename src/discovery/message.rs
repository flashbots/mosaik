use {
	crate::discovery::peer::PeerInfo,
	serde::{Deserialize, Serialize},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CatalogHash {
	hash: Vec<u8>,
}

impl CatalogHash {
	pub(crate) fn hash(&self) -> &[u8] {
		&self.hash
	}

	pub(crate) fn into_bytes(self) -> Vec<u8> {
		rmp_serde::to_vec(&self).expect("catalog hash serialization cannot fail")
	}

	pub(crate) fn from_bytes(
		bytes: &[u8],
	) -> Result<Self, rmp_serde::decode::Error> {
		rmp_serde::from_slice(bytes)
	}
}

impl From<Vec<u8>> for CatalogHash {
	fn from(value: Vec<u8>) -> Self {
		Self { hash: value }
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Catalog {
	// TODO: change to `SignedPeerInfo` and verify on receipt
	peers: Vec<PeerInfo>,
}

impl Catalog {
	pub(crate) fn peers(&self) -> &[PeerInfo] {
		&self.peers
	}

	pub(crate) fn into_bytes(self) -> Vec<u8> {
		rmp_serde::to_vec(&self).expect("catalog serialization cannot fail")
	}

	pub(crate) fn from_bytes(
		bytes: &[u8],
	) -> Result<Self, rmp_serde::decode::Error> {
		rmp_serde::from_slice(bytes)
	}
}

impl From<Vec<PeerInfo>> for Catalog {
	fn from(value: Vec<PeerInfo>) -> Self {
		Self { peers: value }
	}
}
