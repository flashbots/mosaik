//! Mosaik Replicated Collections
//!
//! Each collection instance creates its own mosaik group that runs a raft
//! consensus algorithm. Each collection instance is independent and can be used
//! with different sets of nodes in the cluster.

#![allow(unreachable_code, unused)]

mod depq;
mod map;
mod primitives;
mod set;
mod sync;
mod vec;
mod when;

pub use {
	depq::PriorityQueue,
	map::Map,
	primitives::{StoreId, Version},
	set::Set,
	sync::Config as SyncConfig,
	vec::Vec,
	when::When,
};

const WRITER: bool = true;
const READER: bool = false;

#[derive(Debug, thiserror::Error)]
pub enum Error<T> {
	/// The node is temporarily offline.
	///
	/// The error carries the value that failed to be used in the operation, which
	/// can be retried later when the node is back online.
	#[error("Offline")]
	Offline(T),

	/// The network is permanently down, and the operation cannot be completed.
	///
	/// This is an unrecoverable error and the replicated data structure is no
	/// longer usable.
	#[error("Network is down")]
	NetworkDown,
}
