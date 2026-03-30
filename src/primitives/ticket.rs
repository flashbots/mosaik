use {
	crate::UniqueId,
	bytes::Bytes,
	serde::{Deserialize, Serialize},
};

/// This type represents authentication tokens, credentials, or any other type
/// of data that may be used to authorize a peer.
#[derive(
	Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash,
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
