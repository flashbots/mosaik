//! # Mosaik
//!
//! A Rust SDK for building self-organizing, leaderless distributed systems,
//! providing primitives for automatic discovery, topology management, and
//! load-balancing.

pub mod discovery;
pub mod groups;
pub mod network;
pub mod primitives;
pub mod streams;

// Mosaik Public API entry point
pub use network::{Network, NetworkId};

#[cfg(feature = "test-utils")]
pub mod test_utils;

// Hidden re-exports for common types used by the Public API
#[doc(hidden)]
pub use {
	futures,
	iroh::{self, SecretKey, Signature},
	streams::{Criteria, Datum, StreamId},
};
