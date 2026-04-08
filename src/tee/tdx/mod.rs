mod local;
mod ticket;
mod validator;

pub const TICKET_CLASS: crate::UniqueId =
	crate::id!("mosaik.tee.tdx.ticket.v1");

pub use {local::NetworkTicketExt, validator::TdxValidator};

#[cfg(feature = "tdx-builder")]
mod builder;

#[cfg(feature = "tdx-builder")]
pub use builder::ImageBuilder;
