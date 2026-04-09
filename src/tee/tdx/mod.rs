mod local;
mod measure;
mod ticket;
mod validator;

pub const TICKET_CLASS: crate::UniqueId =
	crate::id!("mosaik.tee.tdx.ticket.v1");

pub use {
	local::NetworkTicketExt,
	measure::*,
	tdx_quote::Quote,
	ticket::{TdxTicketData, TicketError},
	validator::TdxValidator,
};

#[cfg(feature = "tdx-builder")]
mod builder;

/// Builders for creating TEE images from Rust crates, used in `build.rs`
/// scripts.
#[cfg(feature = "tdx-builder")]
pub mod build {

	/// Returns a builder for creating TDX images based on Alpine Linux.
	///
	/// Used in `build.rs` scripts like this:
	///
	/// ```no_run
	/// fn main() {
	/// 	let output = mosaik::tee::tdx::build::alpine().build();
	/// 	println!("cargo:warning=Alpine Image build output: {output:#?}");
	/// }
	/// ```
	#[cfg(feature = "tdx-builder-alpine")]
	pub fn alpine() -> super::builder::alpine::AlpineBuilder {
		super::builder::alpine::AlpineBuilder::default()
	}

	#[cfg(feature = "tdx-builder-alpine")]
	pub use super::builder::alpine::AlpineBuilderOutput;

	/// Returns a builder for creating TDX images based on Ubuntu Linux.
	///
	/// Used in `build.rs` scripts like this:
	///
	/// ```no_run
	/// fn main() {
	/// 	let output = mosaik::tee::tdx::build::ubuntu().build();
	/// 	println!("cargo:warning=Ubuntu Image build output: {output:#?}");
	/// }
	/// ```
	#[cfg(feature = "tdx-builder-ubuntu")]
	pub fn ubuntu() -> super::builder::ubuntu::UbuntuBuilder {
		todo!("Ubuntu builder not implemented yet")
	}

	#[cfg(feature = "tdx-builder-ubuntu")]
	pub use super::builder::ubuntu::UbuntuBuilderOutput;
}
