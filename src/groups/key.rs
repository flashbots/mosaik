use {
	crate::{UniqueId, groups::GroupId},
	core::fmt,
	serde::{Deserialize, Serialize},
};

/// Group Key is used to join a group and authorize membership.
///
/// Notes:
///
/// - Currently only secret-based authentication is implemented.
/// - Group Id is derived by hashing the secret bytes.
/// - During handshake, peers prove knowledge of the secret by hashing the
///   challenge nonce with the secret using [`UniqueId::derive`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GroupKey(UniqueId);

impl GroupKey {
	/// Create a new `GroupKey` from the given secret `UniqueId`.
	pub const fn with_secret(secret: UniqueId) -> Self {
		Self(secret)
	}

	/// Derive the unique identifier for the group from the group key.
	///
	/// For `Secret` keys, the ID is derived by hashing the secret bytes.
	/// For `Token` keys, the ID is derived by hashing the signer's ``PeerId`` and
	/// the group name
	pub fn id(&self) -> GroupId {
		let hash = blake3::hash(self.0.as_bytes());
		GroupId::from_bytes(*hash.as_bytes())
	}

	/// Gets the underlying secret `UniqueId`.
	///
	/// This should never be transmitted over the network, only proofs
	/// of knowledge should be sent.
	pub const fn secret(&self) -> &UniqueId {
		&self.0
	}
}

impl fmt::Display for GroupKey {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}", hex::encode(self.0.as_bytes()))
	}
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("Invalid group key provided")]
	InvalidSecretKey,
}
