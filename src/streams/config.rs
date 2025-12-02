use {
	derive_builder::Builder,
	serde::{Deserialize, Serialize},
};

/// Configuration options for the streams subsystem.
#[derive(Debug, Clone, Builder, Serialize, Deserialize, PartialEq, Hash)]
#[builder(pattern = "owned", setter(prefix = "with"), derive(Debug, Clone))]
#[builder_struct_attr(doc(hidden))]
pub struct Config {
	/// The size of the producer buffer.
	#[builder(default = "1024")]
	pub producer_buffer_size: usize,
}

impl Config {
	/// Creates a new config builder with default values.
	pub fn builder() -> ConfigBuilder {
		ConfigBuilder::default()
	}
}
