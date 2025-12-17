use {
	super::{Catalog, Error, Event, SignedPeerEntry},
	crate::{
		network::{
			LocalNode,
			link::{Link, LinkError, Protocol, Success},
		},
		primitives::{Short, UnboundedChannel},
	},
	core::fmt,
	iroh::{
		EndpointAddr,
		endpoint::Connection,
		protocol::{AcceptError, ProtocolHandler},
	},
	serde::{Deserialize, Serialize},
	tokio::sync::{mpsc::UnboundedReceiver, watch},
};

/// The catalog synchronization protocol exchanges snapshots of the signed peers
/// in the local catalog with remote peers to keep catalogs in sync across the
/// network.
///
/// Notes:
///
/// - During catalog sync peers with the higher entry version take precedence
///   and overwrite stale entries.
///
/// - Each peer's local entry is never overwritten during sync; only remote
///   entries are updated.
///
/// - Unsigned entries are not synced.
pub(super) struct CatalogSync {
	local: LocalNode,
	catalog: watch::Sender<Catalog>,
	events: UnboundedChannel<Event>,
}

impl Protocol for CatalogSync {
	/// ALPN identifier for the catalog sync protocol.
	const ALPN: &'static [u8] = b"/mosaik/discovery/sync/1.0";
}

/// Internal methods
impl CatalogSync {
	/// Creates a new `CatalogSync` protocol handler instance.
	///
	/// This is used internally by the discovery system to handle incoming
	/// catalog synchronization requests and to initiate syncs with remote peers.
	pub fn new(local: LocalNode, catalog: watch::Sender<Catalog>) -> Self {
		Self {
			local,
			catalog,
			events: UnboundedChannel::default(),
		}
	}

	/// Returns a mutable reference to the events receiver.
	///
	/// This is polled by the discovery worker to process incoming events from the
	/// announcement protocol.
	pub const fn events(&mut self) -> &mut UnboundedReceiver<Event> {
		self.events.receiver()
	}

	/// Returns the protocol listener instance responsible for accepting incoming
	/// connections for the catalog sync protocol.
	pub const fn protocol(&self) -> &impl ProtocolHandler {
		self
	}

	/// Initiates a catalog synchronization with the given peer.
	///
	/// This will update the local catalog with any new or updated entries from
	/// the remote peer that are not already present in the local catalog or are
	/// newer than the local entries.
	///
	/// The sync protocol follows this process:
	/// - The initiator connects to the remote peer and opens a bidirectional
	///   stream and sends its local catalog snapshot as [`CatalogSnapshot`].
	///
	/// - The wire-level stream is `Framed` with length-prefixed messages.
	///
	/// - The responder receives the snapshot, and sends back its own local
	///   catalog snapshot as [`CatalogSnapshot`] before merging the received
	///   snapshot into its local catalog.
	///
	/// - Both peers then merge the received snapshots into their local catalogs,
	///   updating or adding entries as necessary based on the versioning rules.
	///
	/// - The initiator closes the connection after receiving the responder's
	///   snapshot
	///
	/// - This async method's lifetime is detached from `self` and can be spawned
	///   as a background task that requires `Send` + `Sync` + `'static`.
	pub fn sync_with(
		&self,
		peer: EndpointAddr,
	) -> impl Future<Output = Result<(), Error>> + Send + 'static {
		let local = self.local.clone();
		let catalog = self.catalog.clone();
		let events_tx = self.events.sender().clone();

		async move {
			tracing::trace!(
				peer = %Short(&peer.id),
				"initiating discovery catalog sync",
			);

			// Establish a direct connection with remote peer on the catalog sync ALPN
			let cancel = local.termination().clone();
			let mut link = local
				.connect_with_cancel::<CatalogSync>(peer.clone(), cancel)
				.await
				.map_err(LinkError::from)?;

			// Send our local catalog snapshot to the remote peer
			let local_snapshot = { (*catalog.borrow()).clone() };
			let local_snapshot = CatalogSnapshot::from(&local_snapshot);
			link
				.send(&local_snapshot)
				.await
				.inspect_err(|e| {
					tracing::warn!(
						peer = %Short(&peer.id),
						error = %e,
						"failed to send local catalog snapshot",
					);
				})
				.map_err(LinkError::from)?;

			// Await the remote peer's catalog snapshot
			// [`SignedPeerEntry`] will implicitly verify the signatures of each
			// received entry.
			let remote_snapshot = link
				.recv::<CatalogSnapshot>()
				.await
				.inspect_err(|e| {
					tracing::warn!(
						peer = %Short(&peer.id),
						error = %e,
						"failed to receive remote catalog snapshot",
					);
				})
				.map_err(LinkError::from)?;

			// Merge the remote snapshot into the local catalog and emit events
			// that reflect the changes made.
			catalog.send_if_modified(|catalog| {
				let remote_catalog_size = remote_snapshot.0.len();
				let local_catalog_size = catalog.iter_signed().count();
				let events = catalog.extend_signed(remote_snapshot.0.into_iter());
				let mut updates = 0;
				let mut insertions = 0;

				for event in events {
					if matches!(event, Event::PeerDiscovered(_)) {
						insertions += 1;
					} else if matches!(event, Event::PeerUpdated(_)) {
						updates += 1;
					}
					let _ = events_tx.send(event);
				}

				tracing::trace!(
					peer = %Short(&peer.id),
					new_peers = %insertions,
					updated_peers = %updates,
					remote_catalog_size = %remote_catalog_size,
					local_catalog_size = %local_catalog_size,
					"full catalog sync complete [initiator]"
				);

				updates > 0 || insertions > 0
			});

			// end of sync, the initiator is responsible for
			// initiating the close of the connection, and the
			// acceptor will await stream closure.
			link.close(Success).await.map_err(LinkError::Close)?;

			Ok(())
		}
	}
}

impl fmt::Debug for CatalogSync {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("CatalogSync").finish()
	}
}

impl ProtocolHandler for CatalogSync {
	fn accept(
		&self,
		connection: Connection,
	) -> impl Future<Output = Result<(), AcceptError>> + Send {
		let cancel = self.local.termination().clone();
		let catalog = self.catalog.clone();

		async move {
			let remote_id = connection.remote_id();
			tracing::trace!(
				peer = %Short(&remote_id),
				"accepting incoming discovery catalog sync",
			);

			// Accept the incoming link for the catalog sync protocol
			let mut link =
				Link::<CatalogSync>::accept_with_cancel(connection, cancel)
					.await
					.inspect_err(|e| {
						tracing::debug!(
							error = %e,
							peer = %Short(&remote_id),
							"failed to accept incoming catalog sync link",
						);
					})?;

			// The acceptor awaits the remote peer's catalog snapshot message first.
			// [`SignedPeerEntry`] will implicitly verify the signatures of each
			// received entry.
			let remote_snapshot = link
				.recv::<CatalogSnapshot>()
				.await
				.inspect_err(|e| {
					tracing::warn!(
						peer = %Short(&remote_id),
						error = %e,
						"failed to receive remote catalog snapshot",
					);
				})
				.map_err(AcceptError::from_err)?;

			// Send our local catalog snapshot to the remote peer
			let local_snapshot = catalog.borrow().clone();
			let local_snapshot = CatalogSnapshot::from(&local_snapshot);
			link
				.send(&local_snapshot)
				.await
				.inspect_err(|e| {
					tracing::warn!(
						peer = %Short(&link.remote_id()),
						error = %e,
						"failed to send local catalog snapshot",
					);
				})
				.map_err(AcceptError::from_err)?;
			drop(local_snapshot);

			// Merge the remote snapshot into the local catalog and emit events
			// that reflect the changes made.
			let catalog = core::pin::pin!(catalog);
			catalog.send_if_modified(|catalog| {
				let remote_catalog_size = remote_snapshot.0.len();
				let local_catalog_size = catalog.iter_signed().count();
				let events = catalog.extend_signed(remote_snapshot.0.into_iter());
				let mut updates = 0;
				let mut insertions = 0;

				for event in events {
					if matches!(event, Event::PeerDiscovered(_)) {
						insertions += 1;
					} else if matches!(event, Event::PeerUpdated(_)) {
						updates += 1;
					}
					self.events.send(event);
				}

				tracing::trace!(
					peer = %Short(link.remote_id()),
					new_peers = %insertions,
					updated_peers = %updates,
					remote_catalog_size = %remote_catalog_size,
					local_catalog_size = %local_catalog_size,
					"full catalog sync complete [acceptor]"
				);

				updates > 0 || insertions > 0
			});

			// Await the link closure initiated by the remote peer
			link.closed().await?;

			Ok(())
		}
	}
}

/// A snapshot of the discovery catalog at a specific point in time containing
/// all signed peer entries. This is a non-optimal synchronization format used
/// for simplicity in the initial implementation of the catalog sync protocol.
///
/// When peers exchange full catalog snapshots, they can compare the entries and
/// update their local catalogs accordingly.
#[derive(Clone, Serialize, Deserialize)]
pub struct CatalogSnapshot(Vec<SignedPeerEntry>);

impl From<&Catalog> for CatalogSnapshot {
	fn from(catalog: &Catalog) -> Self {
		Self(catalog.iter_signed().cloned().collect())
	}
}

impl core::fmt::Debug for CatalogSnapshot {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		write!(f, "CatalogSnapshot[{}]( ", self.0.len())?;
		for entry in &self.0 {
			write!(
				f,
				"(peer {},  version {:?}) ",
				entry.id(),
				entry.update_version()
			)?;
		}
		write!(f, ")")
	}
}
