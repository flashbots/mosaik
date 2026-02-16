//! Traits, helpers and types used across the SDK

mod channel;
mod encoding;
mod fmt;
mod fut;
mod id;
mod iter;
mod queue;

/// Public API re-exported byte types.
pub use bytes::{Bytes, BytesMut};
#[doc(hidden)]
pub use fmt::*;
/// Public API re-exported primitives.
pub use id::{Digest, Tag, UniqueId};
/// Internal primitives.
pub(crate) use {
	channel::UnboundedChannel,
	encoding::{deserialize, serialize, serialize_to_writer},
	fut::BoxPinFut,
	fut::InternalFutureExt,
	iter::IntoIterOrSingle,
	queue::AsyncWorkQueue,
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
