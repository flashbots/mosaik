mod ticket;

pub use ticket::*;

#[cfg(feature = "tdx")]
pub use crate::tee::tdx::Tdx;
