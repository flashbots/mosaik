//! Traits, helpers and types used across the SDK

mod channel;
mod fmt;
mod id;
mod iter;

/// Public API re-exported byte types.
pub use bytes::{Bytes, BytesMut};
#[doc(hidden)]
pub use fmt::*;
/// Public API re-exported primitives.
pub use id::{Tag, UniqueId};
/// Internal primitives.
pub(crate) use {channel::UnboundedChannel, iter::IntoIterOrSingle};

/// Used internally as a sentinel type for generic parameters.
#[doc(hidden)]
pub enum Variant<const U: usize = 0> {}

use {backoff::backoff::Backoff, std::sync::Arc};

pub type BackoffFactory = Arc<
	dyn Fn() -> Box<dyn Backoff + Send + Sync + 'static> + Send + Sync + 'static,
>;
