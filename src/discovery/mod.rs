use {
	crate::{
		IntoIterOrSingle,
		LocalNode,
		PeerId,
		ProtocolProvider,
		discovery::{announce::Announce, worker::WorkerCommand},
	},
	iroh::{
		endpoint::Connection,
		protocol::{AcceptError, ProtocolHandler, RouterBuilder},
	},
	std::sync::Arc,
	sync::CatalogSync,
	tokio::sync::{broadcast, mpsc::UnboundedSender, watch},
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

/// Public Discovery API exports
pub use {
	announce::Event as AnnounceEvent,
	catalog::Catalog,
	config::{Config, ConfigBuilder},
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

	/// Returns a snapshot of the current peers catalog.
	pub fn catalog(&self) -> Catalog {
		self.0.catalog.borrow().clone()
	}

	/// Returns a watch receiver that can be used to monitor changes to the
	/// peers catalog.
	pub fn catalog_watch(&self) -> watch::Receiver<Catalog> {
		self.0.catalog.clone()
	}

	/// Returns a receiver for discovery events.
	///
	/// This receiver will receive all events broadcasted by the discovery system
	/// from this point onward.
	pub fn events(&self) -> broadcast::Receiver<Event> {
		self.0.events.resubscribe()
	}
}

/// Internal construction API
impl Discovery {
	pub(crate) fn new(local: LocalNode, config: Config) -> Self {
		Self(Arc::new(WorkerLoop::spawn(local, config)))
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
	pub(crate) async fn update_local_entry(
		&self,
		update: impl FnOnce(PeerEntry) -> PeerEntry + Send + 'static,
	) {
		self.0.update_local_peer_entry(update).await;
	}
}

// Add all gossip ALPNs to the protocol router
impl ProtocolProvider for Discovery {
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
