use {
	crate::{
		Digest,
		PeerId,
		groups::{GroupId, Groups},
		network::link::Link,
	},
	core::fmt,
	serde::{Deserialize, Serialize},
};

/// Group secret key used during initial handshake to authorize group
/// membership. It's a 32-byte value that is never transmitted over the network
/// directly, only proofs of knowledge derived from it are sent during handshake
/// to authenticate peers.
pub type Secret = Digest;

/// Group secret proof of knowledge.
///
/// This is derived during bonding handshake to prove knowledge of the
/// group secret without transmitting the secret itself over the network.
pub type SecretProof = Digest;

/// Group Key is used to join a group and authorize membership.
///
/// Notes:
///
/// - Currently only secret-based authentication is implemented.
///
/// - Group Id is derived by hashing the secret bytes with the group
///   configuration and the state machine identifier. See [`GroupId`] for more
///   details on how the group id is derived.
///
/// - During handshake, peers prove knowledge of the secret without revealing
///   the secret itself by generating a proof of knowledge derived from the
///   secret and the shared random (see [`Link::shared_random`]) value for the
///   link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GroupKey {
	/// The secret key used to derive the group id and authenticate peers during
	/// bonding handshake.
	secret: Secret,
}

impl GroupKey {
	/// Create a new `GroupKey` from the given secret `UniqueId`.
	pub fn from_secret(secret: impl Into<Secret>) -> Self {
		let secret = secret.into();
		Self { secret }
	}

	/// Creates a new `GroupKey` with a randomly generated secret.
	pub fn random() -> Self {
		Self {
			secret: Secret::random(),
		}
	}

	/// Gets the underlying secret `UniqueId`.
	///
	/// This should never be transmitted over the network, only proofs
	/// of knowledge should be sent.
	pub const fn secret(&self) -> &Secret {
		&self.secret
	}

	/// Validates the validity of the authentication proof contained in this
	/// handshake message against the provided group key and link.
	pub fn validate_proof(
		&self,
		link: &Link<Groups>,
		proof: SecretProof,
		group_id: GroupId,
	) -> bool {
		self.generate_proof(link, link.remote_id(), group_id) == proof
	}

	/// Generates a proof of knowledge of the secret key for the given peer and
	/// link.
	pub fn generate_proof(
		&self,
		link: &Link<Groups>,
		peer_id: PeerId,
		group_id: GroupId,
	) -> SecretProof {
		self
			.secret()
			.derive(link.shared_random(group_id))
			.derive(peer_id.as_bytes())
	}
}

impl fmt::Display for GroupKey {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let head = hex::encode(&self.secret().as_bytes()[..1]);
		let tail = hex::encode(&self.secret().as_bytes()[31..]);
		write!(f, "{head}****{tail}")
	}
}
