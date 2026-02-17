//! Mosaik Replicated Collections

#![allow(unreachable_code, unused)]

mod depq;
mod error;
mod map;
mod set;
mod vec;
mod when;

pub use {
	depq::PriorityQueue,
	error::{InsertError, InsertManyError, RemoveError},
	map::Map,
	set::Set,
	vec::Vec,
	when::When,
};

pub type StoreId = crate::UniqueId;

pub struct Version(pub crate::groups::Index);
