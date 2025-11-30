use {
	derive_builder::Builder,
	serde::{Deserialize, Serialize},
};

/// Configuration options for the streams subsystem.
#[derive(Debug, Clone, Builder, Serialize, Deserialize, PartialEq, Hash)]
#[builder(pattern = "owned", setter(prefix = "with"))]
pub struct Config {}

impl Config {
	/// Creates a new config builder with default values.
	pub fn builder() -> ConfigBuilder {
		ConfigBuilder::default()
	}
}
