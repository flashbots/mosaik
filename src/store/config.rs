use {
	derive_builder::Builder,
	serde::{Deserialize, Serialize},
};

#[derive(Debug, Clone, Builder, Serialize, Deserialize)]
#[builder(pattern = "owned", setter(prefix = "with"), derive(Debug, Clone))]
#[builder_struct_attr(doc(hidden))]
pub struct Config {}

impl Config {
	/// Creates a new config builder with default values.
	pub fn builder() -> ConfigBuilder {
		ConfigBuilder::default()
	}
}
