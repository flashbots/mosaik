//! # Mosaik
//!
//! A Rust SDK for building self-organizing, leaderless distributed systems,
//! providing primitives for automatic discovery, topology management, and
//! load-balancing.

pub mod adapters;
pub mod discovery;
pub mod groups;
pub mod network;
pub mod primitives;
pub mod store;
pub mod streams;

pub use {
	bytes::{Bytes, BytesMut},
	futures,
	groups::{Group, GroupKey, Groups},
	iroh::{self, SecretKey, Signature},
	network::{Network, NetworkId, PeerId},
	primitives::Digest,
	store::{PrimaryStore, ReplicaStore, StoreId},
	streams::{Criteria, Datum, StreamId},
};
