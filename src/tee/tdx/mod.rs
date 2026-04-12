//! # Intel TDX
//!
//! Intel Trust Domain Extensions (TDX) support for mosaik nodes
//! running inside a TDX Trust Domain. This module provides everything
//! needed to generate, carry, and verify hardware attestation quotes
//! within a mosaik network.
//!
//! # Key types
//!
//! | Type | Role |
//! |------|------|
//! | `NetworkTdxExt` | Extension trait on [`Network`](crate::network::Network) for generating TDX tickets from within an enclave |
//! | `TdxTicket` | A mosaik [`Ticket`](crate::primitives::Ticket) wrapping a TDX attestation quote |
//! | `Tdx` | [`TicketValidator`](crate::primitives::TicketValidator) that verifies TDX quotes against expected measurements |
//! | `Quote` | The raw TDX attestation quote (re-exported from `tdx_quote`) |
//!
//! # Usage
//!
//! On the **attesting** side (inside a TDX enclave), generate a ticket
//! and publish it to the discovery catalog:
//!
//! ```rust,ignore
//! use mosaik::*;
//!
//! // Generates a TDX quote binding the node's PeerId
//! let ticket = network.tdx().ticket()?;
//! network.discovery().add_ticket(ticket);
//! ```
//!
//! alternatively, the `install_own_ticket` method can be used to generate and
//! install the ticket in one step:
//!
//! ```rust,ignore
//! if network.tdx().available() {
//! 	network.tdx().install_own_ticket()?;
//! }
//! ```
//!
//! On the **verifying** side, require a valid TDX attestation when
//! joining a group or subscribing to a stream:
//!
//! ```rust,ignore
//! use mosaik::tee::tdx::Tdx;
//!
//! let group1 = network.groups()
//!     .with_key(key)
//!     .with_state_machine(my_machine)
//!     .require_ticket(Tdx::new()
//!         .with_expected_mrtd("91eb2b44d..38873118b7"))
//!     .join();
//!
//! // require the joining node to have the same measurement as us
//! let group2 = network.groups()
//!     .with_key(key)
//!     .with_state_machine(my_machine)
//!     .require_ticket(Tdx::new().with_own_mrtd().expect("tdx support"))
//!     .join();

//! ```

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
