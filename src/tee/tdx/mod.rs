mod local;
mod measure;
mod ticket;
mod validator;

pub const TICKET_CLASS: crate::UniqueId =
	crate::id!("mosaik.tee.tdx.ticket.v1");

pub use {
	local::{Error as NetworkTdxError, NetworkTdxExt},
	measure::*,
	tdx_quote::Quote,
	ticket::{TdxTicket, TdxTicketError},
	validator::Tdx,
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
	/// 	let output = mosaik::tdx::build::alpine().build();
	/// 	println!("cargo:warning=Alpine Image build output: {output:#?}");
	/// }
	/// ```
	#[cfg(feature = "tdx-builder-alpine")]
	pub fn alpine() -> super::builder::alpine::AlpineBuilder {
		super::builder::alpine::AlpineBuilder::default()
	}

	pub use super::builder::BuilderOutput;

	/// Returns a builder for creating TDX images based on Ubuntu Linux.
	///
	/// Used in `build.rs` scripts like this:
	///
	/// ```no_run
	/// fn main() {
	/// 	let output = mosaik::tdx::build::ubuntu().build();
	/// 	println!("cargo:warning=Ubuntu Image build output: {output:#?}");
	/// }
	/// ```
	#[cfg(feature = "tdx-builder-ubuntu")]
	pub fn ubuntu() -> super::builder::ubuntu::UbuntuBuilder {
		super::builder::ubuntu::UbuntuBuilder::default()
	}
}
