#![allow(clippy::doc_markdown)]

//! # Primitives
//!
//! Shared types and traits used across all mosaik subsystems.
//!
//! Key types:
//!
//! - [`Digest`] — a blake3 hash used as the basis for all identifiers
//!   ([`NetworkId`](crate::NetworkId), [`StreamId`](crate::StreamId),
//!   [`GroupId`](crate::GroupId), [`StoreId`](crate::StoreId)).
//! - [`UniqueId`] — a compile-time-friendly identifier derived from a
//!   human-readable string via the [`unique_id!`](crate::unique_id) (alias
//!   [`id!`](crate::id)) macro.
//! - [`Tag`] — a short label attached to a node's discovery entry for
//!   role-based filtering.
//! - [`Datum`] — the marker trait for any type that can be sent over streams or
//!   stored in collections (`Serialize + DeserializeOwned + Clone + Send + Sync
//!   + 'static`).
//! - [`Ticket`] / [`TicketValidator`] — opaque credentials and their
//!   validators, used to gate access to streams and collections (e.g. TEE
//!   attestation quotes).

mod channel;
mod fmt;
mod fut;
mod id;
mod iter;
mod queue;

pub mod datum;
pub mod encoding;

/// Public API re-exported byte types.
pub use bytes::{self, Bytes, BytesMut};
pub use hex;
/// Internal primitives.
pub(crate) use {
	channel::UnboundedChannel,
	encoding::{EncodeError, deserialize, serialize, serialize_to_writer},
	fut::BoxPinFut,
	fut::InternalFutureExt,
	iter::IntoIterOrSingle,
	queue::AsyncWorkQueue,
};
/// Public API re-exported primitives.
pub use {
	datum::{Datum, Encoded},
	fmt::*,
	id::{Digest, Tag, UniqueId},
};

/// Used internally as a sentinel type for generic parameters.
#[doc(hidden)]
pub enum Variant<const U: usize = 0> {}

use {backoff::backoff::Backoff, std::sync::Arc};

pub type BackoffFactory = Arc<
	dyn Fn() -> Box<dyn Backoff + Send + Sync + 'static> + Send + Sync + 'static,
>;

/// Used internally as a sealed trait to prevent external implementations of
/// certain traits.
#[doc(hidden)]
pub(crate) mod sealed {
	pub trait Sealed {}
}
