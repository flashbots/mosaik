//! TDX image builders

#[cfg(feature = "tdx-builder-alpine")]
mod alpine;

#[cfg(feature = "tdx-builder-ubuntu")]
mod ubuntu;

pub struct ImageBuilder;

impl ImageBuilder {
	#[cfg(feature = "tdx-builder-alpine")]
	pub const fn alpine() -> alpine::AlpineBuilder {
		alpine::AlpineBuilder
	}

	#[cfg(feature = "tdx-builder-ubuntu")]
	pub const fn ubuntu() -> ubuntu::UbuntuBuilder {
		ubuntu::UbuntuBuilder
	}
}
