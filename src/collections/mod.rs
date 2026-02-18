//! Mosaik Replicated Collections

#![allow(unreachable_code, unused)]

mod depq;
mod error;
mod map;
mod set;
mod vec;
mod when;

use derive_more::Display;
pub use {
	depq::PriorityQueue,
	error::{InsertError, InsertManyError, RemoveError},
	map::Map,
	set::Set,
	vec::Vec,
	when::When,
};

pub type StoreId = crate::UniqueId;

#[derive(Debug, Copy, Clone, Display)]
pub struct Version(pub crate::groups::Index);

pub trait Value:
	Clone
	+ core::fmt::Debug
	+ serde::Serialize
	+ serde::de::DeserializeOwned
	+ core::hash::Hash
	+ PartialEq
	+ Eq
	+ Send
	+ Sync
	+ Unpin
	+ 'static
{
}

impl<T> Value for T where
	T: Clone
		+ core::fmt::Debug
		+ serde::Serialize
		+ serde::de::DeserializeOwned
		+ core::hash::Hash
		+ PartialEq
		+ Eq
		+ Send
		+ Sync
		+ Unpin
		+ 'static
{
}

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
