//! Mosaik Replicated Collections
//!
//! Each collection instance creates its own mosaik group that runs a raft
//! consensus algorithm. Each collection instance is independent and can be used
//! with different sets of nodes in the cluster.

#![allow(unreachable_code, unused)]

mod depq;
mod map;
mod once;
mod primitives;
mod register;
mod set;
mod sync;
mod vec;
mod when;

pub use {
	depq::{PriorityQueue, PriorityQueueReader, PriorityQueueWriter},
	map::{Map, MapReader, MapWriter},
	once::{Once, OnceReader, OnceWriter},
	primitives::{StoreId, Version},
	register::{Register, RegisterReader, RegisterWriter},
	set::{Set, SetReader, SetWriter},
	sync::Config as SyncConfig,
	vec::{Vec, VecReader, VecWriter},
	when::When,
};

use crate::primitives::EncodeError;

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

	/// An error occurred during encoding the value for replication.
	#[error("Encoding: {1}")]
	Encoding(T, EncodeError),

	/// The network is permanently down, and the operation cannot be completed.
	///
	/// This is an unrecoverable error and the replicated data structure is no
	/// longer usable.
	#[error("Network is down")]
	NetworkDown,
}

/// Trait for collection definitions that provide a reader constructor.
///
/// Implemented automatically by the [`collection!`] macro.
pub trait CollectionReader {
	type Reader;
	fn reader(network: &crate::Network) -> Self::Reader;
}

/// Trait for collection definitions that provide a writer constructor.
///
/// Implemented automatically by the [`collection!`] macro.
pub trait CollectionWriter {
	type Writer;
	fn writer(network: &crate::Network) -> Self::Writer;
}

/// Convenience type alias for the reader type of a collection definition.
pub type ReaderOf<C> = <C as CollectionReader>::Reader;

/// Convenience type alias for the writer type of a collection definition.
pub type WriterOf<C> = <C as CollectionWriter>::Writer;

/// Declares a named collection definition with a compile-time `StoreId`.
///
/// # Syntax
///
/// ```ignore
/// // Full (reader + writer):
/// collection!(pub MyCollection = Vec<String>, "my.store.id");
///
/// // Reader only:
/// collection!(pub reader MyCollection = Vec<String>, "my.store.id");
///
/// // Writer only:
/// collection!(pub writer MyCollection = Vec<String>, "my.store.id");
///
/// // With generics:
/// collection!(pub MyCollection<T> = Vec<T>, "my.store.id");
/// ```
///
/// # Usage
///
/// ```ignore
/// use mosaik::collections::{CollectionReader, CollectionWriter, ReaderOf};
///
/// collection!(pub MyVec = mosaik::collections::Vec<String>, "my.vec");
///
/// struct MyType {
///     reader: ReaderOf<MyVec>,
/// }
///
/// impl MyType {
///     pub fn new(network: &mosaik::Network) -> Self {
///         Self { reader: MyVec::reader(network) }
///     }
/// }
/// ```
#[macro_export]
macro_rules! collection {
	($($tt:tt)*) => {
		$crate::__collection_impl! { @$crate; $($tt)* }
	};
}

/// This trait is implemented for each collection type and allows API users to
/// create instances of the collection reader and writer types from a
/// compile-time definition of the collection.
pub trait CollectionFromDef {
	type Reader;
	type Writer;

	fn reader(network: &crate::Network, store_id: StoreId) -> Self::Reader;
	fn writer(network: &crate::Network, store_id: StoreId) -> Self::Writer;
}

/// A compile-time definition of a collection that can be used to create reader
/// and writer instances for that collection with a predefined `StoreId`.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct CollectionDef<C: CollectionFromDef> {
	pub store_id: StoreId,
	_marker: core::marker::PhantomData<fn(&C)>,
}

impl<C: CollectionFromDef> CollectionDef<C> {
	pub const fn new(store_id: StoreId) -> Self {
		Self {
			store_id,
			_marker: core::marker::PhantomData,
		}
	}

	pub const fn from_reader(reader_def: &ReaderDef<C>) -> Self {
		Self {
			store_id: reader_def.store_id,
			_marker: core::marker::PhantomData,
		}
	}

	pub const fn from_writer(writer_def: &WriterDef<C>) -> Self {
		Self {
			store_id: writer_def.store_id,
			_marker: core::marker::PhantomData,
		}
	}

	pub const fn as_reader(&self) -> ReaderDef<C> {
		ReaderDef::new(self.store_id)
	}

	pub const fn as_writer(&self) -> WriterDef<C> {
		WriterDef::new(self.store_id)
	}

	pub fn reader(&self, network: &crate::Network) -> C::Reader {
		C::reader(network, self.store_id)
	}

	pub fn writer(&self, network: &crate::Network) -> C::Writer {
		C::writer(network, self.store_id)
	}
}

/// A compile-time definition of a collection reader that can be used to create
/// reader instances for a collection with a predefined `StoreId`.
///
/// This is most often exported by libraries that own the writer side of a
/// collection, to allow users to create readers that connect to the writer's
/// collection instance without needing to know the `StoreId`.
pub struct ReaderDef<C: CollectionFromDef> {
	pub store_id: StoreId,
	_marker: core::marker::PhantomData<fn(&C)>,
}

impl<C: CollectionFromDef> ReaderDef<C> {
	pub const fn new(store_id: StoreId) -> Self {
		Self {
			store_id,
			_marker: core::marker::PhantomData,
		}
	}

	pub fn open(&self, network: &crate::Network) -> C::Reader {
		C::reader(network, self.store_id)
	}
}

/// A compile-time definition of a collection writer that can be used to create
/// writer instances for a collection with a predefined `StoreId`.
pub struct WriterDef<C: CollectionFromDef> {
	pub store_id: StoreId,
	_marker: core::marker::PhantomData<fn(&C)>,
}

impl<C: CollectionFromDef> WriterDef<C> {
	pub const fn new(store_id: StoreId) -> Self {
		Self {
			store_id,
			_marker: core::marker::PhantomData,
		}
	}

	pub fn open(&self, network: &crate::Network) -> C::Writer {
		C::writer(network, self.store_id)
	}
}
