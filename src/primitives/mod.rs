mod channel;
mod id;
mod iter;

/// Internal primitives.
pub(crate) use channel::UnboundedChannel;
/// Public API re-exported primitives.
pub use id::{Tag, UniqueId};
pub(crate) use iter::IntoIterOrSingle;

/// Used internally as a sentinel type for generic parameters.
#[doc(hidden)]
pub enum Variant<const U: usize = 0> {}
