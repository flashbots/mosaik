use {
	crate::{Groups, PeerId, UniqueId, groups::GroupId, network::link::Link},
	core::fmt,
};

/// Group Key is used to join a group and authorize membership.
///
/// Notes:
///
/// - Currently only secret-based authentication is implemented.
///
/// - Group Id is derived by hashing the secret bytes with blake3.
///
/// - During handshake, peers prove knowledge of the secret by hashing the
///   challenge nonce with the secret using [`UniqueId::derive`].
#[derive(Debug, Clone, PartialEq)]
pub struct GroupKey {
	/// The secret key used to derive the group id and authenticate peers during
	/// bonding handshake.
	secret: UniqueId,

	/// The public identifier for the group derived from the secret.
	id: GroupId,
}

impl GroupKey {
	/// Create a new `GroupKey` from the given secret `UniqueId`.
	pub fn from_secret(secret: UniqueId) -> Self {
		let hash = blake3::hash(secret.as_bytes());
		let id = GroupId::from_bytes(*hash.as_bytes());
		Self { secret, id }
	}

	/// Derive the unique identifier for the group from the group key.
	/// Currently this is done by hashing the secret bytes, since we only support
	/// secret-based authentication in this iteration of the protocol.
	pub const fn id(&self) -> &GroupId {
		&self.id
	}

	/// Gets the underlying secret `UniqueId`.
	///
	/// This should never be transmitted over the network, only proofs
	/// of knowledge should be sent.
	pub const fn secret(&self) -> &UniqueId {
		&self.secret
	}

	/// Validates the validity of the authentication proof contained in this
	/// handshake message against the provided group key and link.
	pub fn validate_proof(&self, link: &Link<Groups>, proof: UniqueId) -> bool {
		self.generate_proof(link, link.remote_id()) == proof
	}

	/// Generates a proof of knowledge of the secret key for the given peer and
	/// link.
	pub fn generate_proof(
		&self,
		link: &Link<Groups>,
		peer_id: PeerId,
	) -> UniqueId {
		self
			.secret()
			.derive(link.shared_random(self.id()))
			.derive(peer_id.as_bytes())
	}
}

impl fmt::Display for GroupKey {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}", hex::encode(self.id.as_bytes()))
	}
}

impl serde::Serialize for GroupKey {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
	where
		S: serde::Serializer,
	{
		if serializer.is_human_readable() {
			serializer.serialize_str(&hex::encode(self.secret.as_bytes()))
		} else {
			serializer.serialize_bytes(self.secret.as_bytes())
		}
	}
}

impl<'de> serde::Deserialize<'de> for GroupKey {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: serde::Deserializer<'de>,
	{
		struct GroupKeyVisitor;

		impl serde::de::Visitor<'_> for GroupKeyVisitor {
			type Value = GroupKey;

			fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
				formatter.write_str("a valid group secret key")
			}

			fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
			where
				E: serde::de::Error,
			{
				let bytes = hex::decode(v).map_err(serde::de::Error::custom)?;
				let secret = UniqueId::from_bytes(
					bytes
						.try_into()
						.map_err(|_| serde::de::Error::custom("invalid secret length"))?,
				);
				Ok(GroupKey::from_secret(secret))
			}

			fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
			where
				E: serde::de::Error,
			{
				let secret = UniqueId::from_bytes(
					v.try_into()
						.map_err(|_| serde::de::Error::custom("invalid secret length"))?,
				);
				Ok(GroupKey::from_secret(secret))
			}
		}

		if deserializer.is_human_readable() {
			deserializer.deserialize_str(GroupKeyVisitor)
		} else {
			deserializer.deserialize_bytes(GroupKeyVisitor)
		}
	}
}
