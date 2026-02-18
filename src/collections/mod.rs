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
