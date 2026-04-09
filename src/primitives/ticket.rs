use {
	crate::{UniqueId, discovery::PeerEntry},
	bytes::Bytes,
	chrono::{DateTime, Utc},
	core::{cmp::Ordering, time::Duration},
	derive_more::Display,
	serde::{Deserialize, Serialize},
	std::sync::Arc,
};

/// This type represents authentication tokens, credentials, or any other type
/// of data that may be used to authorize a peer.
#[derive(
	Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub struct Ticket {
	/// The unique identifier of the type of ticket.
	///
	/// This is used to determine how to interpret the `data` field and which
	/// authorization scheme to use for validating the ticket.
	pub class: UniqueId,

	/// The opaque data of the ticket. The content and format of this data is
	/// determined by the `class` field and is not interpreted by the discovery
	/// system.
	pub data: Bytes,
}

impl Ticket {
	pub const fn new(class: UniqueId, data: Bytes) -> Self {
		Self { class, data }
	}

	/// A unique identifier for this ticket, derived from its class and data.
	pub fn id(&self) -> UniqueId {
		self.class.derive(&self.data)
	}
}

/// This type represents the expiration policy of a ticket, which determines
/// when a ticket should be reevaluated after successful validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Expiration {
	/// The ticket never expires.
	Never,

	/// The ticket expires at the specified timestamp.
	At(DateTime<Utc>),
}

impl PartialOrd for Expiration {
	fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
		Some(self.cmp(other))
	}
}

impl Ord for Expiration {
	fn cmp(&self, other: &Self) -> Ordering {
		match (self, other) {
			(Self::Never, Self::Never) => Ordering::Equal,
			(Self::Never, _) => Ordering::Greater,
			(_, Self::Never) => Ordering::Less,
			(Self::At(a), Self::At(b)) => a.cmp(b),
		}
	}
}

impl Expiration {
	/// Returns `true` if the ticket has already expired at the current time.
	pub fn is_expired(&self) -> bool {
		match self {
			Self::Never => false,
			Self::At(t) => *t < Utc::now(),
		}
	}

	/// Returns the remaining duration until the ticket expires, or `None` if
	/// the ticket never expires or has already expired.
	pub fn remaining(&self) -> Option<Duration> {
		match self {
			Self::Never => None,
			Self::At(t) => (*t - Utc::now()).to_std().ok(),
		}
	}
}

impl core::fmt::Debug for Ticket {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		f.debug_struct("Ticket")
			.field("class", &self.class)
			.field("data", &hex::encode(&self.data))
			.finish()
	}
}

impl core::fmt::Display for Ticket {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		write!(
			f,
			"Ticket(class={}, data={})",
			self.class,
			humansize::format_size(self.data.len(), humansize::DECIMAL)
		)
	}
}

#[derive(Debug, Clone, Copy, Display, thiserror::Error)]
pub struct InvalidTicket;

/// A trait implemented by types that can validate tickets for peer
/// authorization.
pub trait TicketValidator: Send + Sync + 'static {
	/// Class of tickets that this validator can validate.
	fn class(&self) -> UniqueId;

	/// A unique identifier for the type of the ticket validator and its
	/// configuration.
	///
	/// In Groups, this is used as part of the group id derivation, so all members
	/// of the group must use the same ticket validator type and configuration,
	/// otherwise they will derive different group ids and will not be able to
	/// form a bond connection with each other.
	fn signature(&self) -> UniqueId;

	/// Validates the given ticket for the specified peer.
	fn validate(
		&self,
		ticket: &[u8],
		peer: &PeerEntry,
	) -> Result<Expiration, InvalidTicket>;
}

impl<T: TicketValidator + ?Sized> TicketValidator for Box<T> {
	fn class(&self) -> UniqueId {
		(**self).class()
	}

	fn signature(&self) -> UniqueId {
		(**self).signature()
	}

	fn validate(
		&self,
		ticket: &[u8],
		peer: &PeerEntry,
	) -> Result<Expiration, InvalidTicket> {
		(**self).validate(ticket, peer)
	}
}

impl<T: TicketValidator + ?Sized> TicketValidator for Arc<T> {
	fn class(&self) -> UniqueId {
		(**self).class()
	}

	fn signature(&self) -> UniqueId {
		(**self).signature()
	}

	fn validate(
		&self,
		ticket: &[u8],
		peer: &PeerEntry,
	) -> Result<Expiration, InvalidTicket> {
		(**self).validate(ticket, peer)
	}
}
