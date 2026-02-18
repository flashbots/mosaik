use {
	crate::UniqueId,
	core::{fmt::Debug, hash::Hash},
	derive_more::{Deref, Display},
	serde::{Serialize, de::DeserializeOwned},
};

pub type StoreId = UniqueId;

/// Version of the committed state of a collection.
///
/// Monotonically increasing and can be used to track the progress of the
/// collection's state and synchronize operations with the committed state of
/// the collection's group.
///
/// All mutable operations on all mosaik collections return the version of the
/// state when the mutation is expected to be applied and committed to the group
/// state.
///
/// This allows clients to wait for a specific mutation to be applied and
/// committed to the group state before performing subsequent operations that
/// depend on the mutation, by using the `When` API to wait for the collection
/// to reach at least the returned version.
#[derive(
	Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Deref, Display,
)]
pub struct Version(pub crate::groups::Index);

/// Type requirements for values stored in mosaik collections.
pub trait Value:
	Clone
	+ Debug
	+ Serialize
	+ DeserializeOwned
	+ Hash
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
		+ Debug
		+ Serialize
		+ DeserializeOwned
		+ Hash
		+ PartialEq
		+ Eq
		+ Send
		+ Sync
		+ Unpin
		+ 'static
{
}

/// Type requirements for keys in mosaik collections that support key-value
/// pairs, such as `Map` or `Set` (key, ()).
pub trait Key:
	Clone
	+ Serialize
	+ DeserializeOwned
	+ Hash
	+ PartialEq
	+ Eq
	+ Send
	+ Sync
	+ Unpin
	+ 'static
{
}

impl<T> Key for T where
	T: Clone
		+ Serialize
		+ DeserializeOwned
		+ Hash
		+ PartialEq
		+ Eq
		+ Send
		+ Sync
		+ Unpin
		+ 'static
{
}
