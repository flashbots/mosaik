//! # Trusted Execution Environments
//!
//! Support for running mosaik nodes inside hardware-isolated enclaves.
//! TEE integration lets nodes generate cryptographic attestation
//! proofs that are carried as [`Ticket`](crate::Ticket)s in the
//! discovery catalog, so streams and collections can gate access to
//! peers whose identity and code integrity have been verified by the
//! hardware.
//!
//! # Intel TDX
//!
//! The [`tdx`] module (requires the `tdx` feature) provides:
//!
//! - **Attestation** — generate and verify TDX quotes that bind a
//!   node's identity to its measurement registers (MRTD, RTMR).
//! - **Ticket integration** — [`TdxTicket`](tdx::TdxTicket) wraps a
//!   TDX quote as a mosaik [`Ticket`](crate::Ticket), and
//!   [`Tdx`](tdx::Tdx) implements [`TicketValidator`](crate::primitives::TicketValidator)
//!   for verifying them.
//! - **Image builders** (`tdx-builder-alpine`, `tdx-builder-ubuntu`)
//!   — build-time tools for packaging Rust crates into bootable TDX
//!   guest images, used from `build.rs` scripts:
//!
//!   ```rust,ignore
//!   // build.rs
//!   fn main() {
//!       let output = mosaik::tdx::build::alpine()
//!           .build();
//!       println!("cargo:warning=TDX image: {output:#?}");
//!   }
//!   ```

/// Intel TDX (Trust Domain Extensions) support.
#[cfg(feature = "tdx")]
pub mod tdx;
