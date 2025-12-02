//! Traits, helpers and types used across the SDK

mod channel;
mod id;
mod iter;

/// Public API re-exported primitives.
pub use id::{Tag, UniqueId};
/// Internal primitives.
pub(crate) use {channel::UnboundedChannel, iter::IntoIterOrSingle};

/// Used internally as a sentinel type for generic parameters.
#[doc(hidden)]
pub enum Variant<const U: usize = 0> {}
