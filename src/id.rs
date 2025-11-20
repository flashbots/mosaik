use {
	core::str::FromStr,
	derive_more::{Deref, Display, From, Into},
	iroh_gossip::TopicId,
	rand::{Rng, distr::Alphanumeric},
	serde::{Deserialize, Serialize},
	sha3::{Digest, Sha3_256},
};

/// This type uniquely identifies a network by its name.
///
/// On the protocol level this is represented by a gossip `TopicId` that is a
/// 32-byte sha3-256 hash of the network name bytes.
#[derive(
	Debug,
	Clone,
	PartialEq,
	Eq,
	Hash,
	Serialize,
	Deserialize,
	Deref,
	From,
	Into,
	Display,
)]
pub struct NetworkId(String);

impl NetworkId {
	pub fn new(name: impl Into<String>) -> Self {
		Self(name.into())
	}

	/// Computes the protocol-level topic id for the gossip layer.
	pub fn topic_id(&self) -> TopicId {
		Sha3_256::digest(self.0.as_bytes()).into()
	}

	/// Generates a random network id for testing purposes.
	pub fn random() -> Self {
		let random: String = rand::rng()
			.sample_iter(&Alphanumeric)
			.take(10)
			.map(char::from)
			.collect();

		NetworkId(format!("mosaik{random}"))
	}
}

impl From<&str> for NetworkId {
	fn from(s: &str) -> Self {
		Self::new(s)
	}
}

impl FromStr for NetworkId {
	type Err = core::convert::Infallible;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		Ok(Self::new(s))
	}
}

impl AsRef<[u8]> for NetworkId {
	fn as_ref(&self) -> &[u8] {
		self.0.as_bytes()
	}
}
