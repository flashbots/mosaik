//! # Mosaik
//!
//! A Rust runtime for building self-organizing, leaderless distributed systems.

pub mod collections;
pub mod discovery;
pub mod groups;
pub mod network;
pub mod primitives;
pub mod streams;

pub use {
	bytes::{Bytes, BytesMut},
	futures,
	groups::{
		Consistency,
		Consistency::{Strong, Weak},
		Group,
		GroupId,
		GroupKey,
	},
	iroh::{self, SecretKey, Signature},
	network::{Network, NetworkId, PeerId},
	primitives::{Digest, Tag, UniqueId},
	streams::{Criteria, Datum, StreamId},
};
