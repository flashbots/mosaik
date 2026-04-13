//! Peer authentication via opaque, typed credentials.
//!
//! This module provides the [`Ticket`] type (an opaque credential
//! attached to a peer's discovery entry), the [`TicketValidator`]
//! trait for structured, expiration-aware validation, and the
//! built-in [`Jwt`] validator with [`JwtTicketBuilder`] for issuing
//! and verifying JWT tickets.
//!
//! # Quick start
//!
//! ```ignore
//! use mosaik::tickets::{Jwt, Hs256, JwtTicketBuilder, Expiration};
//!
//! // Validate incoming JWTs
//! let validator = Jwt::with_key(Hs256::new(secret))
//!     .allow_issuer("my-app");
//!
//! // Issue JWT tickets
//! let builder = JwtTicketBuilder::new(Hs256::new(secret))
//!     .issuer("my-app");
//! let ticket = builder.build(&peer_id, Expiration::Never);
//! ```
//!
//! # Supported algorithms
//!
//! | Verifying key | Signing key          | Algorithm               |
//! |---------------|----------------------|-------------------------|
//! | [`Hs256`]     | [`Hs256`]            | HMAC-SHA256 (symmetric) |
//! | [`Hs384`]     | [`Hs384`]            | HMAC-SHA384 (symmetric) |
//! | [`Hs512`]     | [`Hs512`]            | HMAC-SHA512 (symmetric) |
//! | [`Es256`]     | [`Es256SigningKey`]   | ECDSA P-256             |
//! | [`Es256k`]    | [`Es256kSigningKey`]  | ECDSA secp256k1         |
//! | [`Es384`]     | [`Es384SigningKey`]   | ECDSA P-384             |
//! | [`Es512`]     | [`Es512SigningKey`]   | ECDSA P-521             |
//! | [`EdDsa`]     | [`EdDsaSigningKey`]   | Ed25519                 |
//!
//! # TDX hardware attestation
//!
//! With the `tdx` feature enabled, the [`Tdx`] validator verifies
//! Intel TDX hardware attestation quotes — cryptographic proof of
//! the code running inside a trust domain. See the
//! [`tee::tdx`](crate::tee::tdx) module for details.
//!
//! # Custom validators
//!
//! For custom credential schemes, implement [`TicketValidator`]
//! directly. For algorithms not covered by the built-in types,
//! use [`VerifyingKey::custom`] and [`SigningKey::custom`].

mod jwt;
mod ticket;

pub use {jwt::*, ticket::*};

#[cfg(feature = "tdx")]
pub use crate::tee::tdx::Tdx;
