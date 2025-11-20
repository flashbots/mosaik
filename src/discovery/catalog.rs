use {
	super::PeerInfo,
	crate::discovery::peer::{PeerId, SignedPeerInfo},
	bytes::Bytes,
	core::{
		pin::Pin,
		task::{Context, Poll},
	},
	futures::{Stream, StreamExt},
	im::{OrdMap, ordmap::ConsumingIter},
	iroh::EndpointId,
	parking_lot::RwLock,
	std::{collections::HashMap, sync::Arc},
	tokio::sync::broadcast,
	tokio_stream::wrappers::{BroadcastStream, errors::BroadcastStreamRecvError},
};

/// A catalog of discovered nodes and their associated peer info.
///
/// Notes:
///
/// - This type is cheap to clone; all clones refer to the same underlying
///   catalog.
///
/// - This type implements `Stream`, yielding `Event`s when the catalog is
///   updated due to discovery updates.
///
/// - The catalog maintains two separate sets of peer info:
///   - Signed peer info, which has been received from other peers and are
///     signed by their private keys. These are considered more trustworthy and
///     they are the only entries that are synced to other peers.
///   - Unsigned peer info, which has been inserted locally (e.g., via test
///     utilities) or for some other reason. Those entries are available on the
///     local node but are not synced to other peers.
///
/// - The catalog is thread-safe and can be accessed concurrently, each access
///   operation works on the most recent snapshot of the catalog.
pub struct Catalog {
	inner: Arc<Inner>,
}

/// Each clone of the catalog will refer to the same underlying data, and will
/// receive updates when the catalog is changed starting from the time of the
/// clone.
impl Clone for Catalog {
	fn clone(&self) -> Self {
		Self {
			inner: Arc::clone(&self.inner),
		}
	}
}

/// Those events are emitted by the [`Catalog`] when it is updated.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
	/// A new peer has been added.
	New(PeerInfo),
	/// An existing peer has updated its info.
	Updated(PeerInfo),
	/// A peer has been removed.
	Removed(PeerId),

	/// The catalog has changed significantly since last event was polled and not
	/// all individual updates are available (e.g., due to lag, full sync).
	SignificantlyChanged,
}

impl Default for Catalog {
	fn default() -> Self {
		let inner = Arc::new(Inner::new());
		Self { inner }
	}
}

/// Manual catalog manipulation public API.
impl Catalog {
	/// Manually adds a peer info to the catalog locally.
	///
	/// Notes:
	/// - Peers added this way are not signed and will not be synced to other
	///   peers and only used by the local node.
	///
	/// - This will overwrite any existing peer info with the same peer id.
	pub fn insert(&self, peer: impl Into<PeerInfo>) {
		let peer = peer.into();
		let id = peer.id();
		let mut signed_writer = self.inner.signed.write();
		let mut unsigned_writer = self.inner.unsigned.write();
		let mut is_update = false;

		if signed_writer.remove(id).is_some() {
			is_update = true;
		}

		if unsigned_writer.insert(*id, peer.clone()).is_some() {
			is_update = true;
		}

		drop(signed_writer);
		drop(unsigned_writer);

		// notify observers about change to the catalog
		if is_update {
			let _ = self.inner.sender.send(Event::Updated(peer));
		} else {
			let _ = self.inner.sender.send(Event::New(peer));
		}
	}

	/// Removes a manually added peer info from the catalog.
	///
	/// This will not remove signed peer infos received from other peers.
	pub fn remove(&self, peer_id: &PeerId) {
		let mut writer = self.inner.unsigned.write();
		if let Some(removed) = writer.remove(peer_id) {
			drop(writer); // release lock asap

			// notify observers about change to the catalog
			let _ = self.inner.sender.send(Event::Removed(*removed.id()));
		}
	}

	pub(crate) fn merge(&mut self, other: &[PeerInfo]) {
		for peer in other.iter() {
			self.insert(peer.clone());
		}
	}
}

/// Read-only catalog public API.
impl Catalog {
	/// Returns an iterator over all known peer infos in the catalog.
	///
	/// Notes:
	/// - This iterator yields both signed and unsigned peer infos.
	/// - This iterator works on a snapshot of the catalog at the time of calling
	///   this method, and is not affected by subsequent updates to the catalog.
	pub fn peers(&self) -> impl Iterator<Item = PeerInfo> + 'static {
		let signed_snapshot = {
			let reader = self.inner.signed.read();
			reader.clone()
		};
		let unsigned_snapshot = {
			let reader = self.inner.unsigned.read();
			reader.clone()
		};

		Iter {
			signed_iter: signed_snapshot.into_iter(),
			unsigned_iter: unsigned_snapshot.into_iter(),
		}
	}

	/// Returns the number of peer infos in the catalog.
	pub fn len(&self) -> usize {
		let signed_reader = self.inner.signed.read();
		let unsigned_reader = self.inner.unsigned.read();
		signed_reader.len() + unsigned_reader.len()
	}

	/// Returns true if the catalog is empty.
	pub fn is_empty(&self) -> bool {
		self.len() == 0
	}

	/// Returns a stream of catalog update events starting from now.
	pub fn watch(&self) -> Events {
		Events {
			buffer: HashMap::new(),
			receiver: BroadcastStream::new(self.inner.sender.subscribe()),
		}
	}

	/// Gets latest full `PeerInfo` entry by `PeerId`
	///
	/// This will lookup signed entries first and fall back to unsigned if not
	/// found.
	pub fn get(&self, id: &PeerId) -> Option<PeerInfo> {
		self
			.inner
			.signed
			.read()
			.get(id)
			.map(|p| p.info.clone())
			.or_else(|| self.inner.unsigned.read().get(id).cloned())
	}

	/// Gets latest full `SignedPeerInfo` entry by `PeerId`.
	pub fn get_signed(&self, id: &PeerId) -> Option<SignedPeerInfo> {
		self.inner.signed.read().get(id).cloned()
	}

	// TODO: change this to a merkle root
	pub(crate) async fn hash(&self) -> Bytes {
		use sha3::Digest as _;
		let mut hasher = sha3::Sha3_256::new();
		for peer in self.peers() {
			hasher.update(peer.digest().to_vec());
		}
		hasher.finalize().to_vec().into()
	}
}

/// Iterator over peer infos in a snapshot of the catalog.
pub struct Iter {
	signed_iter: ConsumingIter<(EndpointId, SignedPeerInfo)>,
	unsigned_iter: ConsumingIter<(EndpointId, PeerInfo)>,
}

impl Iterator for Iter {
	type Item = PeerInfo;

	fn next(&mut self) -> Option<Self::Item> {
		if let Some((_, signed)) = self.signed_iter.next() {
			Some(signed.info)
		} else if let Some((_, unsigned)) = self.unsigned_iter.next() {
			Some(unsigned)
		} else {
			None
		}
	}
}

impl DoubleEndedIterator for Iter {
	fn next_back(&mut self) -> Option<Self::Item> {
		if let Some((_, unsigned)) = self.unsigned_iter.next_back() {
			Some(unsigned)
		} else if let Some((_, signed)) = self.signed_iter.next_back() {
			Some(signed.info)
		} else {
			None
		}
	}
}

impl ExactSizeIterator for Iter {
	fn len(&self) -> usize {
		self.signed_iter.len() + self.unsigned_iter.len()
	}
}

struct Inner {
	signed: RwLock<OrdMap<EndpointId, SignedPeerInfo>>,
	unsigned: RwLock<OrdMap<EndpointId, PeerInfo>>,
	sender: broadcast::Sender<Event>,
}

impl Inner {
	pub fn new() -> Self {
		let (sender, _updates) = broadcast::channel(32);

		Self {
			signed: RwLock::new(im::OrdMap::new()),
			unsigned: RwLock::new(im::OrdMap::new()),
			sender,
		}
	}
}

/// Returns a stream that coalesces consecutive events for the same peer.
///
/// # Coalescing Rules
///
/// Events are buffered and coalesced according to these rules:
///
/// | Buffered Event | New Event    | Result           |
/// |----------------|--------------|------------------|
/// | New            | Updated      | New (latest)     |
/// | New            | Removed      | *(cancelled)*    |
/// | Updated        | Updated      | Updated (latest) |
/// | Updated        | Removed      | Removed          |
/// | Removed        | New          | Updated          |
/// | Removed        | Updated      | *(invalid)*      |
/// | Removed        | Removed      | Removed          |
/// | *(none)*       | New          | New              |
/// | *(none)*       | Updated      | Updated          |
/// | *(none)*       | Removed      | Removed          |
///
/// Special case: `Event::SignificantlyChanged` immediately flushes the buffer
/// and is yielded immediately.
pub struct Events {
	receiver: BroadcastStream<Event>,
	buffer: HashMap<PeerId, Event>,
}

impl Stream for Events {
	type Item = Event;

	fn poll_next(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<Option<Self::Item>> {
		let this = self.get_mut();

		// Try to read all immediately available events and buffer them
		loop {
			match this.receiver.poll_next_unpin(cx) {
				Poll::Ready(Some(Ok(event))) => {
					match event {
						Event::SignificantlyChanged => {
							// Clear buffer and immediately return this event
							this.buffer.clear();
							return Poll::Ready(Some(Event::SignificantlyChanged));
						}
						Event::New(info) => {
							match this.buffer.get(info.id()) {
								None => {
									// Rule: (none) + New = New
									this.buffer.insert(*info.id(), Event::New(info));
								}
								Some(Event::Removed(_)) => {
									// Rule: Removed + New = Updated
									this.buffer.insert(*info.id(), Event::Updated(info));
								}
								Some(Event::New(_)) | Some(Event::Updated(_)) => {
									// Invalid state: shouldn't get New after New/Updated
									// Keep the buffer as-is (first event wins)
								}
								Some(Event::SignificantlyChanged) => {
									// Should never happen (SignificantlyChanged clears buffer)
									unreachable!()
								}
							}
						}
						Event::Updated(info) => {
							match this.buffer.get(info.id()) {
								None => {
									// Rule: (none) + Updated = Updated
									this.buffer.insert(*info.id(), Event::Updated(info));
								}
								Some(Event::New(_)) => {
									// Rule: New + Updated = New (with latest info)
									this.buffer.insert(*info.id(), Event::New(info));
								}
								Some(Event::Updated(_)) => {
									// Rule: Updated + Updated = Updated (with latest info)
									this.buffer.insert(*info.id(), Event::Updated(info));
								}
								Some(Event::Removed(_)) => {
									// Invalid state: shouldn't get Updated after Removed
									// Keep Removed in buffer
								}
								Some(Event::SignificantlyChanged) => {
									// Should never happen (SignificantlyChanged clears buffer)
									unreachable!()
								}
							}
						}
						Event::Removed(peer_id) => {
							match this.buffer.get(&peer_id) {
								None => {
									// Rule: (none) + Removed = Removed
									this.buffer.insert(peer_id, Event::Removed(peer_id));
								}
								Some(Event::New(_)) => {
									// Rule: New + Removed = (cancelled)
									this.buffer.remove(&peer_id);
								}
								Some(Event::Updated(_)) => {
									// Rule: Updated + Removed = Removed
									this.buffer.insert(peer_id, Event::Removed(peer_id));
								}
								Some(Event::Removed(_)) => {
									// Rule: Removed + Removed = Removed
									// Already in buffer, no change needed
								}
								Some(Event::SignificantlyChanged) => {
									// Should never happen (SignificantlyChanged clears buffer)
									unreachable!()
								}
							}
						}
					}
				}
				Poll::Ready(Some(Err(BroadcastStreamRecvError::Lagged(_)))) => {
					// Clear buffer and return SignificantlyChanged
					this.buffer.clear();
					return Poll::Ready(Some(Event::SignificantlyChanged));
				}
				Poll::Ready(None) => {
					// Stream ended, flush buffer
					if let Some((_, event)) = this.buffer.drain().next() {
						return Poll::Ready(Some(event));
					}
					return Poll::Ready(None);
				}
				Poll::Pending => {
					// No more events immediately available, yield one from buffer
					if let Some(peer_id) = this.buffer.keys().next().copied() {
						let event = this.buffer.remove(&peer_id).unwrap();
						return Poll::Ready(Some(event));
					}
					return Poll::Pending;
				}
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use {
		super::*,
		crate::streams::StreamId,
		serde::{Deserialize, Serialize},
	};

	#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
	struct Data1(pub String);

	#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
	struct Data2(pub String);

	#[test]
	fn manual_insert_and_remove() {
		let catalog = Catalog::default();

		assert!(catalog.is_empty());
		assert_eq!(catalog.len(), 0);

		let iter = catalog.peers();
		assert_eq!(iter.count(), 0);

		let peer_info1 = PeerInfo::random();
		let peer_info2 = PeerInfo::random();

		catalog.insert(peer_info1.clone());
		assert!(!catalog.is_empty());
		assert_eq!(catalog.len(), 1);

		let iter = catalog.peers();
		assert_eq!(iter.count(), 1);

		catalog.insert(peer_info2.clone());
		assert!(!catalog.is_empty());
		assert_eq!(catalog.len(), 2);

		let iter = catalog.peers();
		assert_eq!(iter.count(), 2);

		catalog.remove(peer_info1.id());
		assert!(!catalog.is_empty());
		assert_eq!(catalog.len(), 1);

		let iter = catalog.peers();
		assert_eq!(iter.count(), 1);

		catalog.remove(peer_info2.id());
		assert!(catalog.is_empty());
		assert_eq!(catalog.len(), 0);

		let iter = catalog.peers();
		assert_eq!(iter.count(), 0);
	}

	#[tokio::test]
	async fn events_watch_smoke() {
		let catalog = Catalog::default();
		let mut events = catalog.watch();

		assert!(catalog.is_empty());
		assert_eq!(catalog.len(), 0);

		let iter = catalog.peers();
		assert_eq!(iter.count(), 0);

		let peer_info1 = PeerInfo::random();
		let peer_info2 = PeerInfo::random();

		catalog.insert(peer_info1.clone());
		assert_eq!(events.next().await, Some(Event::New(peer_info1.clone())));

		catalog.insert(peer_info2.clone());
		assert_eq!(events.next().await, Some(Event::New(peer_info2.clone())));

		catalog.remove(peer_info1.id());
		assert_eq!(events.next().await, Some(Event::Removed(*peer_info1.id())));

		catalog.remove(peer_info2.id());
		assert_eq!(events.next().await, Some(Event::Removed(*peer_info2.id())));
	}

	#[tokio::test]
	async fn events_watch_coalescing_new() {
		let catalog = Catalog::default();
		let mut events = catalog.watch();

		let peer_info1 = PeerInfo::random();

		catalog.insert(peer_info1.clone());
		catalog.insert(peer_info1.clone());
		catalog.insert(peer_info1.clone());

		assert_eq!(events.next().await, Some(Event::New(peer_info1.clone())));
	}

	#[tokio::test]
	async fn events_watch_coalescing_new_updated() {
		let catalog = Catalog::default();
		let mut events = catalog.watch();

		let peer_info = PeerInfo::random();
		catalog.insert(peer_info.clone());

		// Simulate update with new info but same ID
		let peer_info2 = PeerInfo::new_with_streams(peer_info.address().clone(), {
			let mut streams = peer_info.streams().clone();
			streams.insert(StreamId::of::<Data1>());
			streams
		});

		catalog.insert(peer_info2.clone());

		// Rule: New + Updated = New (with latest info)
		assert_eq!(events.next().await, Some(Event::New(peer_info2.clone())));
	}

	#[tokio::test]
	async fn events_watch_coalescing_new_removed() {
		let catalog = Catalog::default();
		let mut events = catalog.watch();

		let peer_info = PeerInfo::random();
		catalog.insert(peer_info.clone());
		catalog.remove(peer_info.id());

		// Rule: New + Removed = (cancelled)
		// Should get nothing or timeout - use try_next with timeout
		tokio::time::timeout(std::time::Duration::from_millis(50), events.next())
			.await
			.unwrap_err();
	}

	#[tokio::test]
	async fn events_watch_coalescing_updated_updated() {
		let catalog = Catalog::default();
		let mut events = catalog.watch();

		let peer_info = PeerInfo::random();
		catalog.insert(peer_info.clone());
		assert_eq!(events.next().await, Some(Event::New(peer_info.clone())));

		let peer_info2 = PeerInfo::new_with_streams(peer_info.address().clone(), {
			let mut streams = peer_info.streams().clone();
			streams.insert(StreamId::of::<Data1>());
			streams
		});

		catalog.insert(peer_info2.clone());

		let peer_info3 =
			PeerInfo::new_with_streams(peer_info2.address().clone(), {
				let mut streams = peer_info2.streams().clone();
				streams.insert(StreamId::of::<Data2>());
				streams
			});

		catalog.insert(peer_info3.clone()); // Update again

		// Rule: Updated + Updated = Updated (with latest info)
		assert_eq!(
			events.next().await,
			Some(Event::Updated(peer_info3.clone()))
		);
	}

	#[tokio::test]
	async fn events_watch_coalescing_updated_removed() {
		let catalog = Catalog::default();
		let mut events = catalog.watch();

		let peer_info = PeerInfo::random();
		catalog.insert(peer_info.clone());
		assert_eq!(events.next().await, Some(Event::New(peer_info.clone())));

		catalog.insert(peer_info.clone()); // Update
		catalog.remove(peer_info.id());

		// Rule: Updated + Removed = Removed
		assert_eq!(events.next().await, Some(Event::Removed(*peer_info.id())));
	}

	#[tokio::test]
	async fn events_watch_coalescing_removed_new() {
		let catalog = Catalog::default();
		let mut events = catalog.watch();

		let peer_info = PeerInfo::random();
		catalog.insert(peer_info.clone());
		assert_eq!(events.next().await, Some(Event::New(peer_info.clone())));

		catalog.remove(peer_info.id());
		catalog.insert(peer_info.clone()); // Re-add

		// Rule: Removed + New = Updated
		assert_eq!(events.next().await, Some(Event::Updated(peer_info.clone())));
	}

	#[tokio::test]
	async fn events_watch_coalescing_removed_removed() {
		let catalog = Catalog::default();
		let mut events = catalog.watch();

		let peer_info = PeerInfo::random();
		catalog.insert(peer_info.clone());
		assert_eq!(events.next().await, Some(Event::New(peer_info.clone())));

		catalog.remove(peer_info.id());
		catalog.remove(peer_info.id()); // Remove again (no-op)

		// Rule: Removed + Removed = Removed
		assert_eq!(events.next().await, Some(Event::Removed(*peer_info.id())));
	}

	#[tokio::test]
	async fn events_watch_significantly_changed_flushes_buffer() {
		let catalog = Catalog::default();
		let mut events = catalog.watch();

		let peer_info = PeerInfo::random();
		catalog.insert(peer_info.clone());

		// Trigger SignificantlyChanged by making the receiver lag
		// Fill up the channel buffer (32 events)
		for _ in 0..35 {
			catalog.insert(PeerInfo::random());
		}

		// First event should be SignificantlyChanged due to lag
		assert_eq!(events.next().await, Some(Event::SignificantlyChanged));
	}

	#[tokio::test]
	async fn events_watch_multiple_peers_independent() {
		let catalog = Catalog::default();
		let mut events = catalog.watch();

		let peer_info1 = PeerInfo::random();
		let peer_info2 = PeerInfo::random();

		catalog.insert(peer_info1.clone());
		catalog.insert(peer_info2.clone());

		// Each peer should get independent events
		let event1 = events.next().await;
		let event2 = events.next().await;

		assert!(matches!(event1, Some(Event::New(_))));
		assert!(matches!(event2, Some(Event::New(_))));
	}

	#[tokio::test]
	async fn events_watch_complex_coalescing_sequence() {
		let catalog = Catalog::default();
		let mut events = catalog.watch();

		let peer_info = PeerInfo::random();

		// New -> Updated -> Updated -> Removed = (cancelled)
		catalog.insert(peer_info.clone());
		let peer_info2 = PeerInfo::new_with_streams(peer_info.address().clone(), {
			let mut streams = peer_info.streams().clone();
			streams.insert(StreamId::of::<Data1>());
			streams
		});
		catalog.insert(peer_info2.clone());

		let peer_info3 =
			PeerInfo::new_with_streams(peer_info2.address().clone(), {
				let mut streams = peer_info2.streams().clone();
				streams.insert(StreamId::of::<Data2>());
				streams
			});
		catalog.insert(peer_info3.clone());
		catalog.remove(peer_info.id());

		// Should be cancelled (no event)
		tokio::time::timeout(std::time::Duration::from_millis(50), events.next())
			.await
			.unwrap_err();
	}
}
