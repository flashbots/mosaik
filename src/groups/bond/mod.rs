use {
	crate::{
		Digest,
		Groups,
		PeerId,
		discovery::SignedPeerEntry,
		groups::{
			Error,
			bond::worker::Command,
			consensus::ConsensusMessage,
			error::InvalidProof,
			state::WorkerState,
		},
		network::{
			CloseReason,
			UnknownPeer,
			link::{Link, RecvError},
		},
		primitives::Short,
	},
	core::fmt,
	iroh::endpoint::ApplicationClose,
	protocol::{BondMessage, HandshakeEnd},
	std::sync::Arc,
	tokio::{
		sync::{
			mpsc::{UnboundedReceiver, UnboundedSender},
			watch,
		},
		time::timeout,
	},
	tokio_util::sync::CancellationToken,
	worker::BondWorker,
};

mod heartbeat;
mod protocol;
mod worker;

pub(super) use protocol::{Acceptor, HandshakeStart};
use {
	bincode::{config::standard, serde::encode_into_std_write},
	bytes::{BufMut, Bytes, BytesMut},
};

/// A unique identifier for a bond connection between the local node and a
/// remote peer in the group. This value is derived from the shared random
/// between the two peers during bond establishment and is used to correlate the
/// bond connection with the corresponding peer entry in the discovery catalog
/// and to identify the bond in logs and metrics.
///
/// The bond id should be identical on both sides of the bond connection.
pub type BondId = Digest;

#[derive(Debug, Clone)]
pub enum BondEvent {
	Connected,
	ConsensusMessage(ConsensusMessage),
	Terminated(ApplicationClose),
}

pub type BondEvents = UnboundedReceiver<BondEvent>;

/// Handle to a bond with another peer in a group.
#[derive(Clone)]
pub struct Bond {
	/// A unique identifier for this bond connection.
	///
	/// This value is identical on both sides of the bond connection.
	id: BondId,

	/// Cancellation token for terminating the bond's main loop
	/// and to check for cancellation status.
	cancel: CancellationToken,

	/// Channel for sending commands to the bond worker loop.
	commands_tx: UnboundedSender<Command>,

	/// Watch channel for observing updates to the remote peer's discovery entry.
	peer: watch::Receiver<SignedPeerEntry>,
}

impl fmt::Display for Bond {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			f,
			"Bond(peer={}, id={})",
			Short(self.peer.borrow().id()),
			Short(self.id)
		)
	}
}

/// Public API
impl Bond {
	/// Closes the bond connection to the remote peer with the provided
	/// application-level close reason.
	pub async fn close(self, reason: impl CloseReason) {
		let _ = self.commands_tx.send(Command::Close(reason.into()));
		self.cancel.cancelled().await;
	}

	/// Returns the most recent peer entry for the remote peer in the bond.
	pub fn peer(&self) -> SignedPeerEntry {
		self.peer.borrow().clone()
	}

	/// Returns the unique identifier for this bond connection.
	/// This value is derived from the shared random between the two peers during
	/// bond establishment and is used to correlate the bond connection with the
	/// corresponding peer entry in the discovery catalog and to identify the bond
	/// in logs and metrics.
	pub const fn id(&self) -> BondId {
		self.id
	}
}

/// Internal API for creating and accepting bonds
impl Bond {
	/// Initiates the process of creating a new bond connection to a remote
	/// peer in the group.
	///
	/// This happens in response to discovering a new peer in the group via
	/// the discovery catalog. This method is called only for peers that are
	/// already known in the discovery catalog.
	pub(super) async fn create(
		group: Arc<WorkerState>,
		peer: SignedPeerEntry,
	) -> Result<(Self, BondEvents), Error> {
		tracing::trace!(
			network = %group.network_id(),
			peer = %Short(peer.id()),
			group = %Short(group.group_id()),
			"initiating peer bond",
		);

		// attempt to establish a new wire link to the remote peer
		let mut link = group
			.local
			.connect_with_cancel::<Groups>(
				peer.address().clone(),
				group.cancel.child_token(),
			)
			.await
			.map_err(|e| Error::Link(e.into()))?;

		// prepare a handshake message with our proof of knowledge of the group
		// secret and send it to the remote peer over the newly established link
		link
			.send(&HandshakeStart {
				network_id: *group.network_id(),
				group_id: *group.group_id(),
				proof: group.generate_key_proof(&link),
				bonds: group.bonds.iter().map(|b| b.peer()).collect(),
			})
			.await
			.map_err(|e| Error::Link(e.into()))?;

		// After sending our handshake with a proof of knowledge of the group
		// secret, wait for the accepting peer to respond with its own proof of
		// knowledge of the group secret.
		let Ok(recv_result) = timeout(
			group.global_config.handshake_timeout,
			link.recv::<HandshakeEnd>(),
		)
		.await
		else {
			tracing::debug!(
				network = %group.network_id(),
				peer = %Short(peer.id()),
				group = %Short(group.group_id()),
				"handshake timeout waiting for bond confirmation",
			);

			return Err(Error::Link(RecvError::Cancelled.into()));
		};

		let confirm = match recv_result {
			Ok(resp) => resp,
			Err(e) => match e.close_reason() {
				// the remote peer closed the link during handshake because the local
				// node is not known in its discovery catalog. Trigger full discovery
				// catalog sync.
				Some(reason) if reason == UnknownPeer => {
					// trigger full catalog sync and retry bonding
					group
						.discovery
						.sync_with(peer.address().clone())
						.await
						.map_err(Error::Discovery)?;

					// retry creating the bond after syncing the catalog
					return Box::pin(Self::create(group, peer)).await;
				}
				// The remote peer rejected our authentication proof.
				// This is an unrecoverable error in the current set of authorization
				// schemes and most likely indicates an incompatible version of the peer
				// or a malicious actor.
				Some(reason) if reason == InvalidProof => {
					tracing::warn!(
						network = %group.network_id(),
						peer = %Short(peer.id()),
						group = %Short(group.group_id()),
						"remote peer rejected bond handshake due to invalid proof",
					);

					link
						.close(InvalidProof)
						.await
						.map_err(|e| Error::Link(e.into()))?;

					return Err(Error::InvalidGroupKeyProof);
				}
				// Bonding failed for some other reason.
				_ => return Err(Error::Link(e.into())),
			},
		};

		// validate the accepting peer's proof of knowledge of the group secret
		if !group.validate_key_proof(&link, confirm.proof) {
			tracing::warn!(
				network = %group.network_id(),
				peer = %Short(peer.id()),
				group = %Short(group.group_id()),
				"remote peer provided invalid group secret proof",
			);

			link
				.close(InvalidProof)
				.await
				.map_err(|e| Error::Link(e.into()))?;

			return Err(Error::InvalidGroupKeyProof);
		}

		// attempt to connect to all existing peers in the group known to the
		// accepting node
		for peer in confirm.bonds {
			group.bond_with(peer);
		}

		Ok(BondWorker::spawn(group, peer, link))
	}

	/// Accepts an incoming bond connection for this group.
	///
	/// This is called by the group's protocol handler when a new connection
	/// is established  in [`bond::Acceptor::accept`].
	///
	/// By the time this method is called:
	/// - The network id has already been verified to match the local node's
	///   network id.
	/// - The group id has already been verified to match this group's id.
	/// - The presence of the remote peer in the local discovery catalog is
	///   verified.
	/// - The authentication proof has not been verified yet.
	pub(super) async fn accept(
		group: Arc<WorkerState>,
		link: Link<Groups>,
		peer: SignedPeerEntry,
		handshake: HandshakeStart,
	) -> Result<(Self, BondEvents), Error> {
		let mut link = link;

		// verify the remote peer's proof of knowledge of the group secret
		if !group.validate_key_proof(&link, handshake.proof) {
			tracing::warn!(
				network = %group.network_id(),
				peer = %Short(peer.id()),
				group = %Short(group.group_id()),
				"remote peer provided invalid group secret proof",
			);

			link
				.close(InvalidProof)
				.await
				.map_err(|e| Error::Link(e.into()))?;

			return Err(Error::InvalidGroupKeyProof);
		}

		// After verifying the remote peer's proof of knowledge of the group secret,
		// respond with our own proof of knowledge of the group secret.
		let proof = group.generate_key_proof(&link);
		let existing = group.bonds.iter().map(|b| b.peer()).collect();
		let resp = HandshakeEnd {
			proof,
			bonds: existing,
		};
		link.send(&resp).await.map_err(|e| Error::Link(e.into()))?;

		// attempt to connect to all existing peers in the group known to the
		// initiating node
		for peer in handshake.bonds {
			group.bond_with(peer);
		}

		Ok(BondWorker::spawn(group, peer, link))
	}
}

/// Internal Bond API used by the groups module.
impl Bond {
	/// Sends a wire message over the bond connection to the remote peer.
	#[expect(dead_code)]
	pub(super) fn send_message(&self, message: BondMessage) {
		let _ = self.commands_tx.send(Command::SendMessage(message));
	}

	/// Sends a raw pre-encoded message over the bond connection to the remote
	/// peer.
	///
	/// SAFETY:
	/// This method is private and is used only by specialized internal methods in
	/// the `Bonds` type that serialize a `BondMessage` once and then send the
	/// same encoded bytes buffer to all bonded peers. External APIs cannot send
	/// arbitrary bytes buffers and can only send structured `BondMessage` values
	/// via the `send_message` method, which ensures that only valid `BondMessage`
	/// values are sent.
	unsafe fn send_raw_message(&self, message: Bytes) {
		let _ = self.commands_tx.send(Command::SendRawMessage(message));
	}
}

/// A watchable collection of currently active bonds in a group.
///
/// This type allows observing changes to the set of active bonds.
#[derive(Clone)]
pub struct Bonds(pub(super) watch::Sender<im::OrdMap<PeerId, Bond>>);

/// Public API
impl Bonds {
	/// Returns the number of active bonds in the group.
	pub fn len(&self) -> usize {
		self.0.borrow().len()
	}

	/// Returns `true` if there are no active bonds in the group.
	pub fn is_empty(&self) -> bool {
		self.0.borrow().is_empty()
	}

	/// Returns `true` if there is an active bond to the specified peer.
	pub fn contains_peer(&self, peer_id: &PeerId) -> bool {
		self.0.borrow().contains_key(peer_id)
	}

	/// Returns an iterator over all active bonds in the group ordered by their
	/// peer ids at the time of calling this method.
	pub fn iter(&self) -> impl Iterator<Item = Bond> {
		let bonds = self.0.borrow().clone();
		bonds.into_iter().map(|(_, bond)| bond)
	}

	/// Returns a future that resolves when there is a change to the active
	/// bonds in the group.
	pub async fn changed(&self) {
		let _ = self.0.subscribe().changed().await;
	}

	/// Returns the bond to the specified peer if it exists.
	pub fn get(&self, peer_id: &PeerId) -> Option<Bond> {
		self.0.borrow().get(peer_id).cloned()
	}
}

/// Internal API
impl Default for Bonds {
	fn default() -> Self {
		Self(watch::Sender::new(im::OrdMap::new()))
	}
}

/// Internal API
impl Bonds {
	pub(super) fn update_with(
		&self,
		f: impl FnOnce(&mut im::OrdMap<PeerId, Bond>),
	) {
		self.0.send_if_modified(|active| {
			let before = active.len();
			f(active);
			active.len() != before
		});
	}

	/// Notifies all active bonds that the local peer's discovery entry has
	/// been updated.
	pub(super) fn notify_local_info_update(&self, entry: &SignedPeerEntry) {
		self.broadcast(BondMessage::PeerEntryUpdate(Box::new(entry.clone())), &[]);
	}

	/// Notifies all active bonds that a new bond has been formed with the
	/// specified peer and sends its latest known discovery entry.
	pub(super) fn notify_bond_formed(&self, with: &SignedPeerEntry) {
		self.broadcast(BondMessage::BondFormed(Box::new(with.clone())), &[
			*with.id()
		]);
	}

	/// Broadcast a message to all connected peers in the group.
	fn broadcast(&self, message: BondMessage, except: &[PeerId]) {
		// serialize once and reuse a pointer to the same encoded message bytes
		// buffer for all bonds
		let mut writer = BytesMut::new().writer();
		encode_into_std_write(message, &mut writer, standard())
			.expect("BondMessage encoding failed");
		let encoded = writer.into_inner().freeze();

		for bond in self.iter() {
			if !except.contains(bond.peer().id()) {
				unsafe { bond.send_raw_message(encoded.clone()) };
			}
		}
	}
}
