use {
	derive_builder::Builder,
	iroh::{EndpointId, SecretKey},
	serde::{Deserialize, Serialize},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
	secret: SecretKey,
	known_members: Vec<EndpointId>,
}

impl Group {
	pub fn new(secret: SecretKey, known_members: Vec<EndpointId>) -> Self {
		Self {
			secret,
			known_members,
		}
	}

	/// Returns the secret key of the group.
	pub fn secret(&self) -> &SecretKey {
		&self.secret
	}

	/// Returns the known members of the group.
	pub fn known_members(&self) -> &[EndpointId] {
		&self.known_members
	}
}

/// Configuration options for the groups subsystem.
#[derive(Debug, Clone, Builder, Serialize, Deserialize)]
#[builder(pattern = "owned", setter(prefix = "with"), derive(Debug, Clone))]
#[builder_struct_attr(doc(hidden))]
pub struct Config {
	#[builder(setter(into), default = "vec![]")]
	pub groups_to_join: Vec<Group>,
}

impl Config {
	/// Creates a new config builder with default values.
	pub fn builder() -> ConfigBuilder {
		ConfigBuilder::default()
	}
}
