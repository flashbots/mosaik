//! Automatic topology discovery

use {
	crate::{
		discovery::{announce::Announce, worker::WorkerCommand},
		network::{LocalNode, PeerId, link::Protocol},
		primitives::IntoIterOrSingle,
	},
	iroh::{
		EndpointAddr,
		endpoint::Connection,
		protocol::{AcceptError, ProtocolHandler, RouterBuilder},
	},
	std::sync::Arc,
	sync::CatalogSync,
	tokio::sync::{broadcast, mpsc::UnboundedSender, oneshot, watch},
	worker::{Handle, WorkerLoop},
};

mod announce;
mod catalog;
mod config;
mod entry;
mod error;
mod event;
mod sync;
mod worker;

pub use {
	announce::Event as AnnounceEvent,
	catalog::Catalog,
	config::{Config, ConfigBuilder, ConfigBuilderError, IntoConfig},
	entry::{PeerEntry, PeerEntryVersion, SignedPeerEntry},
	error::Error,
	event::Event,
};

/// The discovery system for a Mosaik network.
///
/// The discovery system is composed of two main protocols:
///
/// - The announcement protocol that peers use to broadcast their presence and
///   changes to their own state and metadata in real time as they happen.
///
/// - The synchronization protocol that peers use to exchange and synchronize
///   their catalogs of known peers and their associated metadata.
///
/// - The discovery system maintains a local catalog of known peers and their
///   metadata, which is updated through gossip messages and catalog syncs.
///
/// - This type is cloneable and can be shared across different components of
///   the network stack that need access to discovery functionality.
#[derive(Clone)]
pub struct Discovery(Arc<Handle>);

/// Public API
impl Discovery {
	/// Dials the given peers to initiate connections and discovery processes.
	///
	/// This is an async and best-effort operation; there is no guarantee that the
	/// dial will succeed or that the peers are online.
	pub async fn dial<V>(&self, peers: impl IntoIterOrSingle<PeerId, V>) {
		self.0.dial(peers.iterator().into_iter()).await;
	}

	/// Returns the latest version of the signed peer entry for the local node.
	pub fn me(&self) -> SignedPeerEntry {
		self.catalog().local().clone()
	}

	/// Returns a snapshot of the current peers catalog.
	pub fn catalog(&self) -> Catalog {
		self.0.catalog.borrow().clone()
	}

	/// Returns a watch receiver that can be used to monitor changes to the
	/// peers catalog from this point onwards.
	pub fn catalog_watch(&self) -> watch::Receiver<Catalog> {
		self.0.catalog.subscribe()
	}

	/// Returns a receiver for discovery events.
	///
	/// This receiver will receive all events broadcasted by the discovery system
	/// from this point onward.
	pub fn events(&self) -> broadcast::Receiver<Event> {
		self.0.events.resubscribe()
	}

	/// Performs a full catalog synchronization with the specified peer.
	///
	/// This async method resolves when the sync is complete or fails.
	pub async fn sync_with(
		&self,
		peer_id: impl Into<EndpointAddr>,
	) -> Result<(), Error> {
		let peer_id = peer_id.into();
		let (tx, rx) = oneshot::channel();
		self
			.0
			.commands
			.send(WorkerCommand::SyncWith(peer_id, tx))
			.map_err(|_| Error::Cancelled)?;
		rx.await.map_err(|_| Error::Cancelled)?
	}

	/// Inserts an unsigned [`PeerEntry`] into the local catalog.
	///
	/// Notes:
	/// - This peer entry is not synced to other peers and is only used locally by
	///   this node. When a signed peer entry with the same [`PeerId`] already
	///   exists, this entry is ignored.
	/// - When a signed entry is later added for the same [`PeerId`], the unsigned
	///   entry is removed.
	/// - Returns `true` if the entry was added.
	/// - Insertion fails for the local node's own peer entry.
	/// - Insertion fails for entries that already exist as signed entries.
	pub fn insert(&self, entry: impl Into<PeerEntry>) -> bool {
		self
			.0
			.catalog
			.send_if_modified(|catalog| catalog.insert_unsigned(entry.into()))
	}

	/// Removes an unsigned [`PeerEntry`] from the local catalog by its
	/// [`PeerId`] and returns `true` if the entry was removed.
	///
	/// Notes:
	/// - This will only remove unsigned entries that were previously added via
	///   [`Discovery::insert`]. Signed entries are not affected.
	pub fn remove(&self, peer_id: PeerId) -> bool {
		self
			.0
			.catalog
			.send_if_modified(|catalog| catalog.remove_unsigned(&peer_id).is_some())
	}

	/// Clears all unsigned [`PeerEntry`] instances from the local catalog that
	/// were manually added via [`Discovery::insert`]. Signed entries are not
	/// affected. Returns `true` if any entries were removed.
	pub fn clear_unsigned(&self) -> bool {
		self
			.0
			.catalog
			.send_if_modified(|catalog| catalog.clear_unsigned())
	}
}

/// Internal construction API
impl Discovery {
	pub(crate) fn new(local: LocalNode, config: Config) -> Self {
		Self(WorkerLoop::spawn(local, config))
	}
}

/// Internal mutation API
impl Discovery {
	/// Updates the local peer entry using the provided update function.
	///
	/// If the update results in a change to the local entry contents, it is
	/// re-signed and broadcasted to the network which respectively updates their
	/// catalogues.
	///
	/// This api is not intended to be used directly by users of the discovery
	/// system, but rather by higher-level abstractions that manage the local
	/// peer's state.
	pub(crate) fn update_local_entry(
		&self,
		update: impl FnOnce(PeerEntry) -> PeerEntry + Send + 'static,
	) {
		self.0.catalog.send_modify(|catalog| {
			let local_entry = catalog.local().clone();
			let updated_entry = update(local_entry.into());
			let signed_updated_entry = updated_entry
				.sign(self.0.local.secret_key())
				.expect("signing updated local peer entry failed.");

			assert!(
				catalog.upsert_signed(signed_updated_entry).is_ok(),
				"local peer info versioning error. this is a bug."
			);
		});
	}
}

// Add all gossip ALPNs to the protocol router
impl crate::network::ProtocolProvider for Discovery {
	fn install(&self, protocols: RouterBuilder) -> RouterBuilder {
		let announce = Acceptor {
			name: Announce::ALPN,
			variant_fn: WorkerCommand::AcceptAnnounce,
			tx: self.0.commands.clone(),
		};

		let catalog_sync = Acceptor {
			name: CatalogSync::ALPN,
			variant_fn: WorkerCommand::AcceptCatalogSync,
			tx: self.0.commands.clone(),
		};

		protocols
			.accept(announce.name, announce)
			.accept(catalog_sync.name, catalog_sync)
	}
}

struct Acceptor {
	name: &'static [u8],
	variant_fn: fn(Connection) -> WorkerCommand,
	tx: UnboundedSender<WorkerCommand>,
}

impl core::fmt::Debug for Acceptor {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		// SAFETY: ALPNs are always valid UTF-8 hardcoded at compile time
		write!(f, "Discovery({})", unsafe {
			str::from_utf8_unchecked(self.name)
		})
	}
}

impl ProtocolHandler for Acceptor {
	async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
		let command = (self.variant_fn)(connection);
		let _ = self.tx.send(command);
		Ok(())
	}
}
