use {super::SyncConfig, crate::primitives::TicketValidator, std::sync::Arc};

/// Configuration for a collection instance.
///
/// Controls both the snapshot synchronization behavior and the ticket-based
/// authentication requirements for the underlying group. All members of a
/// collection must use the same configuration, otherwise they will derive
/// different group ids and will not be able to see each other's changes.
///
/// # Examples
///
/// ```ignore
/// use mosaik::collections::{CollectionConfig, SyncConfig};
///
/// // Default configuration (no auth, default sync):
/// let config = CollectionConfig::default();
///
/// // Custom sync + ticket auth:
/// let config = CollectionConfig::default()
///     .with_sync(SyncConfig::default().with_fetch_batch_size(5000))
///     .require_ticket(my_validator);
/// ```
#[derive(Clone, Default)]
pub struct CollectionConfig {
	pub(crate) sync: SyncConfig,
	pub(crate) auth: Vec<Arc<dyn TicketValidator>>,
}

impl CollectionConfig {
	/// Sets the snapshot synchronization configuration.
	///
	/// Different sync configurations produce different group ids — all
	/// members of the collection must use the same sync configuration.
	#[must_use]
	pub const fn with_sync(mut self, sync: SyncConfig) -> Self {
		self.sync = sync;
		self
	}

	/// Adds a ticket validator for authenticating peers that attempt to
	/// join the collection's underlying group. Peers must satisfy all
	/// configured validators to be allowed to participate. Can be called
	/// multiple times to require multiple types of tickets.
	///
	/// Ticket validators contribute to the derived group id — all members
	/// of the collection must have the same validators in the same order.
	#[must_use]
	pub fn require_ticket(mut self, validator: impl TicketValidator) -> Self {
		self.auth.push(Arc::new(validator));
		self
	}
}

impl From<SyncConfig> for CollectionConfig {
	fn from(sync: SyncConfig) -> Self {
		Self {
			sync,
			..Default::default()
		}
	}
}
