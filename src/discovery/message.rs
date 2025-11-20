use {
	crate::discovery::peer::PeerInfo,
	bytes::Bytes,
	serde::{Deserialize, Serialize},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum DiscoveryMessage {
	CatalogHashCompareRequest(CatalogHashCompareRequest),
	CatalogHashCompareResponse(CatalogHashCompareResponse),
}

impl From<CatalogHashCompareRequest> for DiscoveryMessage {
	fn from(value: CatalogHashCompareRequest) -> Self {
		Self::CatalogHashCompareRequest(value)
	}
}

impl From<CatalogHashCompareResponse> for DiscoveryMessage {
	fn from(value: CatalogHashCompareResponse) -> Self {
		Self::CatalogHashCompareResponse(value)
	}
}

impl DiscoveryMessage {
	pub(crate) fn into_bytes(self) -> Vec<u8> {
		rmp_serde::to_vec(&self)
			.expect("discovery message serialization cannot fail")
	}

	pub(crate) fn from_bytes(
		bytes: &[u8],
	) -> Result<Self, rmp_serde::decode::Error> {
		rmp_serde::from_slice(bytes)
	}
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CatalogHashCompareRequest {
	hash: Bytes,
}

impl CatalogHashCompareRequest {
	pub(crate) fn hash(&self) -> &[u8] {
		&self.hash
	}

	pub(crate) fn into_bytes(self) -> Vec<u8> {
		rmp_serde::to_vec(&self)
			.expect("catalog hash compare request serialization cannot fail")
	}

	pub(crate) fn from_bytes(
		bytes: &[u8],
	) -> Result<Self, rmp_serde::decode::Error> {
		rmp_serde::from_slice(bytes)
	}
}

impl From<Bytes> for CatalogHashCompareRequest {
	fn from(value: Bytes) -> Self {
		Self { hash: value }
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum CatalogHashCompareResponse {
	Matches,
	Mismatches(Catalog),
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
	fn from(peers: Vec<PeerInfo>) -> Self {
		Self { peers }
	}
}

impl FromIterator<PeerInfo> for Catalog {
	fn from_iter<T: IntoIterator<Item = PeerInfo>>(iter: T) -> Self {
		let peers: Vec<PeerInfo> = iter.into_iter().collect();
		Self { peers }
	}
}
