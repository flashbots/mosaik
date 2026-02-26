use {
	super::Config,
	crate::{
		NetworkId,
		PeerId,
		discovery::Catalog,
		network::LocalNode,
		primitives::{FmtIter, Short, UnboundedChannel},
	},
	arrayvec::ArrayVec,
	core::time::Duration,
	futures::{StreamExt, TryStreamExt, stream::SelectAll},
	iroh::{KeyParsingError, address_lookup::AddressLookup},
	pkarr::{
		dns::{Name, ResourceRecord, rdata::TXT},
		errors::SignedPacketBuildError,
		*,
	},
	rand::prelude::SliceRandom,
	std::collections::HashSet,
	tokio::{
		sync::{
			mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
			watch,
		},
		time::{MissedTickBehavior, interval, sleep},
	},
};

/// Maximum number of retries when a CAS (compare-and-swap) conflict occurs
/// during DHT publish. This prevents infinite retry loops when many nodes are
/// racing to update the same record.
const MAX_CAS_RETRIES: usize = 3;

/// Delay between CAS retries when a publish conflict occurs. This gives some
/// time for the competing publisher to finish and reduces the likelihood of
/// repeated conflicts.
const CAS_ATTEMPT_DELAY: Duration = Duration::from_secs(5);

/// Interval for polling the DHT for peer records when the catalog has at least
/// one known peer.
const RELAXED_POLL_INTERVAL: Duration = Duration::from_secs(60);

/// Poll interval used when the local catalog has no known peers.
///
/// Once at least one peer appears in the catalog, the poll interval relaxes to
/// the configured [`Config::dht_poll_interval`]. If peers later disconnect and
/// the catalog empties again, polling switches back to this aggressive rate.
const AGGRESSIVE_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Interval for publishing the local peer's presence to the DHT.
const PUBLISH_INTERVAL: Duration = Duration::from_secs(300);

/// TTL for a peer record in the DHT - 1h.
const PEER_RECORD_TTL: Duration = Duration::from_secs(3600);

/// Automatic bootstrap mechanism for Mosaik networks.
///
/// Automatic bootstrap works by inserting the local peer id as a TXT record in
/// the [Mainline DHT](https://en.wikipedia.org/wiki/Mainline_DHT) under a public key
/// that is derived from the network id this node is connected to.
///
/// This allows nodes to discover some initial peers to connect to when they
/// first join the network, without having to run dedicated bootstrap nodes or
/// hardcode any peer ids or address.
pub struct DhtBootstrap {
	events: UnboundedChannel<PeerId>,
	healthy_tx: UnboundedSender<PeerId>,
	unhealthy_tx: UnboundedSender<PeerId>,
}

impl DhtBootstrap {
	pub(super) fn new(
		local: &LocalNode,
		config: &Config,
		catalog: watch::Receiver<Catalog>,
	) -> Self {
		let events = UnboundedChannel::default();
		let (healthy_tx, healthy_rx) = unbounded_channel();
		let (unhealthy_tx, unhealthy_rx) = unbounded_channel();

		if config.no_auto_bootstrap {
			tracing::debug!(
				network = %local.network_id(),
				"DHT auto bootstrap is disabled"
			);
		} else {
			let events_tx = events.sender().clone();

			let worker = WorkerLoop {
				events_tx,
				catalog,
				healthy_rx,
				unhealthy_rx,
				local: local.clone(),
				healthy_peers: HashSet::new(),
				unhealthy_peers: HashSet::new(),
			};

			let cancel = local.termination().child_token();
			tokio::spawn(cancel.run_until_cancelled_owned(worker.run()));
		}

		Self {
			events,
			healthy_tx,
			unhealthy_tx,
		}
	}

	pub const fn events(&mut self) -> &mut UnboundedReceiver<PeerId> {
		self.events.receiver()
	}

	pub fn note_healthy(&self, peer_id: PeerId) {
		let _ = self.healthy_tx.send(peer_id);
	}

	pub fn note_unhealthy(&self, peer_id: PeerId) {
		let _ = self.unhealthy_tx.send(peer_id);
	}
}

struct WorkerLoop {
	local: LocalNode,
	events_tx: UnboundedSender<PeerId>,
	healthy_rx: UnboundedReceiver<PeerId>,
	unhealthy_rx: UnboundedReceiver<PeerId>,
	catalog: watch::Receiver<Catalog>,
	healthy_peers: HashSet<PeerId>,
	unhealthy_peers: HashSet<PeerId>,
}

impl WorkerLoop {
	/// Main loop: runs two independent cycles on separate intervals.
	///
	/// - **Publish**: writes the local peer's entry to the DHT record so other
	///   nodes can discover us. Fires at [`Config::dht_publish_interval`].
	/// - **Poll**: reads the DHT record to discover other peers. Starts with an
	///   aggressive interval ([`AGGRESSIVE_POLL_INTERVAL`]) when the catalog has
	///   no peers, and relaxes to [`Config::dht_poll_interval`] once connectivity
	///   is established. Automatically switches back to aggressive polling if all
	///   peers disconnect.
	async fn run(mut self) {
		let Ok(client) = Client::builder()
			.no_relays()
			.cache_size(0)
			.build()
			.inspect_err(|e| {
				tracing::warn!(
					error = %e,
					network = %self.local.network_id(),
					"failed to initialize DHT bootstrap client"
				);
			})
		else {
			return;
		};

		self.mark_healthy(self.local.id());

		let network_record = NetworkRecord::new(*self.local.network_id());
		let mut publish_tick = interval(with_jitter(PUBLISH_INTERVAL));
		publish_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);

		// start with an initial publish to announce our presence and poll
		// existing peers as soon as possible.
		self.publish_cycle(&client, &network_record).await;

		loop {
			tokio::select! {
				// periodically update the dht record with the current set
				// of known healthy peers.
				_ = publish_tick.tick() => {
					self.publish_cycle(&client, &network_record).await;
				}

				// Periodically poll new entries from the DHT bootstrap set
				// and feed them into the local discovery subsystem.
				() = sleep(self.next_poll_delay()) => {
					self.poll_cycle(&client, &network_record).await;
				}

				// Received a signal from other parts of the discovery subsystem that
				// a peer has successfully been reached and is healthy.
				Some(peer_id) = self.healthy_rx.recv() => {
					self.mark_healthy(peer_id);
				}

				// Received a signal from other parts of the discovery subsystem that
				// there was a failure to reach this peer.
				Some(peer_id) = self.unhealthy_rx.recv() => {
					self.mark_unhealthy(peer_id);
				}
			}
		}
	}

	/// Returns the delay until the next poll cycle. Uses aggressive polling
	/// when the catalog has no known peers, otherwise falls back to the
	/// configured poll interval.
	fn next_poll_delay(&self) -> Duration {
		if self.catalog.borrow().signed_peers_count() == 0 {
			// We want to find a peer asap, poll aggressively.
			// This is most often the case when the whole network is instantiated at
			// the same time during a deployment or similar event.
			AGGRESSIVE_POLL_INTERVAL
		} else {
			// normal operating conditions, poll at reduced frequency just to keep the
			// local discovery subsystem updated with any new peers that may have
			// appeared in the DHT.
			RELAXED_POLL_INTERVAL
		}
	}

	/// Called periodically to publish the local peer's presence to the DHT and
	/// garbage collect unhealthy peers. This also implicitly fetches the latest
	/// version of the bootstrap set.
	///
	/// This fetches the current bootstrap set from the DHT, validates all the
	/// peers in it, removes any unhealthy peers, and adds the local peer if
	/// there's room in the set.
	async fn publish_cycle(&mut self, client: &Client, network: &NetworkRecord) {
		for _ in 0..MAX_CAS_RETRIES {
			let mut existing = BootstrapSet::fetch(client, network).await;

			tracing::info!(
				network = %network.network_id,
				peers = %FmtIter::<Short<_>, _>::new(existing.peers().collect::<Vec<_>>()),
				"current bootstrap set in DHT",
			);

			// feed all newly discovered healthy peers into the local discovery
			// subsystem so they can be dialed and added to the local catalog.
			for peer in self.validate_peers(existing.peers()).await {
				let _ = self.events_tx.send(peer).ok();
			}

			// remove any unhealthy peers from the bootstrap set
			let mut modified = existing.remove_all(&self.unhealthy_peers);

			// If there's room in the bootstrap set, populate it with
			// peer ids from the healthy set until we reach capacity.
			let mut healthy: Vec<_> = self.healthy_peers.iter().copied().collect();
			healthy.shuffle(&mut rand::rng());
			modified |= existing.try_populate(healthy.into_iter());

			if modified {
				match existing.publish(client, network).await {
					Ok(()) => {
						// successfully published the updated bootstrap set to the DHT.
						tracing::info!(
							network = %network.network_id,
							peers = %FmtIter::<Short<_>, _>::new(existing.peers().collect::<Vec<_>>()),
							"published updated bootstrap set to Mainline DHT"
						);
						return;
					}
					Err(PublishError::PublishError(
						pkarr::errors::PublishError::Concurrency(_),
					)) => {
						// lost the race to update the DHT record, retry publishing up to
						// the max number of retries.
						tracing::debug!(
							network = %network.network_id,
							"publishing bootstrap set to DHT failed due to concurrent modification, retrying..."
						);

						sleep(with_jitter(CAS_ATTEMPT_DELAY)).await;
					}
					Err(e) => {
						// unrecoverable error, skip this publish cycle and try again at the
						// next interval.
						tracing::warn!(
							error = %e,
							network = %network.network_id,
							"failed to publish bootstrap set to DHT",
						);
						return;
					}
				}
			}
		}
	}

	/// Periodically polls the most recent version of the `BootstrapSet` from the
	/// DHT and feeds any new entries into the local discovery subsystem.
	///
	/// Polling has two frequencies: when the local catalog has no known peers,
	/// polling is more aggressive to facilitate faster bootstrapping. Once at
	/// least one peer is known, polling relaxes to the configured interval to
	/// reduce load on the DHT and avoid unnecessary churn.
	async fn poll_cycle(&mut self, client: &Client, network: &NetworkRecord) {
		let dht_set = BootstrapSet::fetch(client, network).await;

		// feed all newly discovered healthy peers into the local discovery
		// subsystem so they can be dialed and added to the local catalog.
		for peer in self.validate_peers(dht_set.peers()).await {
			let _ = self.events_tx.send(peer).ok();
		}
	}

	/// Validates the given peer id by checking if we are able to resolve its
	/// address from its `PeerId`.
	///
	/// Returns an iterator of the newly discovered healthy peers that were
	/// validated successfully and not previously marked as healthy.
	async fn validate_peers(
		&mut self,
		peers: impl Iterator<Item = PeerId>,
	) -> impl Iterator<Item = PeerId> + 'static {
		let mut healthy = HashSet::<PeerId>::new();
		let mut merged = SelectAll::new();
		for peer in peers {
			if let Some(resolver) =
				self.local.endpoint().address_lookup().resolve(peer)
			{
				merged.push(resolver.map_ok(move |_| peer).map_err(move |_| peer));
			}
		}

		while let Some(result) = merged.next().await {
			match result {
				Ok(peer_id) => {
					if !self.healthy_peers.contains(&peer_id) {
						healthy.insert(peer_id);
					}
					self.mark_healthy(peer_id);
				}
				Err(peer_id) => self.mark_unhealthy(peer_id),
			}
		}

		healthy.into_iter()
	}

	/// Marks a peer as healthy, making it a viable candidate to be included in
	/// the `BootstrapSet` published to the DHT.
	fn mark_healthy(&mut self, peer_id: PeerId) {
		self.healthy_peers.insert(peer_id);
		self.unhealthy_peers.remove(&peer_id);
	}

	/// Marks a peer as unhealthy, preventing it from being included in the
	/// `BootstrapSet` published to the DHT and making it a candidate for eviction
	/// if it is already present there.
	fn mark_unhealthy(&mut self, peer_id: PeerId) {
		if peer_id != self.local.id() {
			self.healthy_peers.remove(&peer_id);
			self.unhealthy_peers.insert(peer_id);
		}
	}
}

/// Adds random jitter (up to 1/3 of the base duration) to desynchronize
/// concurrent publishers.
fn with_jitter(base: Duration) -> Duration {
	let max_jitter_secs = base.as_secs() / 3 + 1;
	let jitter = Duration::from_secs(rand::random_range(0..max_jitter_secs));
	base + jitter
}

/// Represents one network's record in the DHT, which includes the network ID
/// and the keypair used to sign the record.
struct NetworkRecord {
	network_id: NetworkId,
	keypair: Keypair,
	record_suffix: String,
	public_key: PublicKey,
}

impl NetworkRecord {
	/// Creates a new `NetworkRecord` for the given network ID, deriving the
	/// keypair from the network ID bytes.
	pub fn new(network_id: NetworkId) -> Self {
		let keypair = Keypair::from_secret_key(network_id.as_bytes());
		let public_key = keypair.public_key();
		let record_suffix = format!(".{public_key}");

		Self {
			network_id,
			keypair,
			record_suffix,
			public_key,
		}
	}

	/// Returns the public key corresponding to this network record, which is used
	/// as the DHT record key for bootstrapping this network.
	pub const fn public_key(&self) -> &PublicKey {
		&self.public_key
	}

	pub const fn keypair(&self) -> &Keypair {
		&self.keypair
	}

	/// Returns the expected suffix of peer record names for this network.
	///
	/// Peer records in the DHT have the format
	/// `<base58-peer-id>.<network-public-key>`.
	pub const fn record_suffix(&self) -> &str {
		self.record_suffix.as_str()
	}
}

/// Represents a single peer record in the DHT bootstrap set.
///
/// This is represented as a TXT record in the DHT.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
struct PeerRecord {
	peer_id: PeerId,
}

impl PeerRecord {
	/// Parses a `PeerRecord` from a DNS TXT entry.
	///
	/// Expects the name to be of the form
	/// `<base58-peer-id>.<network-public-key>`.
	pub fn parse(
		record: &ResourceRecord<'_>,
		network: &NetworkRecord,
	) -> Result<Self, ParseError> {
		let name = record.name.to_string();

		let peer_id_b58 = name
			.strip_suffix(network.record_suffix())
			.ok_or(ParseError::InvalidSuffix)?;

		let peer_id_bytes = b58::decode(peer_id_b58)?;
		let peer_id_bytes: [u8; 32] = peer_id_bytes
			.try_into()
			.map_err(|bytes: Vec<u8>| ParseError::InvalidPeerIdLen(bytes.len()))?;

		let peer_id = PeerId::from_bytes(&peer_id_bytes)?;

		Ok(Self { peer_id })
	}

	/// Encode the peer id as base58 so it fits within the TXT record length limit
	/// of 63 characters. The resulting txt record will have the format
	/// `<base58-peer-id>.<network-public-key>`.
	pub fn encoded_name(&self) -> String {
		b58::encode(self.peer_id.as_bytes())
	}
}

/// This is the final set of peers that are published in the DHT record under
/// this `NetworkId`.
///
/// The Mainline DHT has a maximum record size of 1000 bytes, so we need to
/// chose peers that are well-connected and purge disconnected peers. Note that
/// every member of the network has the same view and write permissions to the
/// DHT record.
///
/// TODO:
/// consider a two-tier approach where `NetworkId` is the public key of an
/// ed25519 keypair and only nodes possessing the private key may write to the
/// DHT record, currently the DHT record's public key is derived from the
/// publicly known `NetworkId`.
struct BootstrapSet {
	records: ArrayVec<PeerRecord, { Self::MAX_RECORDS }>,
	timestamp: Option<Timestamp>,
}

impl BootstrapSet {
	/// Maximum number of peer entries to maintain in a single DHT bootstrap
	/// record.
	///
	/// Individual Mainline DHT records are capped at 1000 bytes total (including
	/// DNS headers, all resource records, and pkarr framing). Each peer entry is
	/// a TXT record whose name is the base58-encoded peer ID (~44 chars), with
	/// DNS label encoding and type/class/TTL/rdlength overhead adding up to
	/// ~58–100 bytes per entry.
	///
	/// A cap of 10 leaves sufficient headroom (~200 bytes) below the 1000-byte
	/// limit to avoid encoding-related edge cases, while still providing more
	/// than enough peers for any node to bootstrap from.
	const MAX_RECORDS: usize = 10;

	/// Returns `true` if this bootstrap set has reached the maximum number of
	/// peer records and cannot accommodate any more.
	pub const fn is_full(&self) -> bool {
		self.records.len() >= Self::MAX_RECORDS
	}

	/// Returns an iterator over the peer IDs in this bootstrap set.
	pub fn peers(&self) -> impl Iterator<Item = PeerId> {
		self.records.iter().map(|r| r.peer_id)
	}

	/// Removes multiple peers from the bootstrap set.
	///
	/// Returns `true` if any peer was removed, or `false` if the set was not
	/// modified.
	pub fn remove_all(&mut self, peer_ids: &HashSet<PeerId>) -> bool {
		let prev_len = self.records.len();
		self.records.retain(|r| !peer_ids.contains(&r.peer_id));
		prev_len != self.records.len()
	}

	/// Attempts to insert a peer into the bootstrap set. Returns `true` if the
	/// peer was inserted, or `false` the set was not modified.
	pub fn try_insert(&mut self, peer_id: PeerId) -> bool {
		if self.is_full() {
			// no more room for new peers.
			return false;
		}

		if self.records.iter().any(|r| r.peer_id == peer_id) {
			// already present, no need to insert again.
			return false;
		}

		self.records.push(PeerRecord { peer_id });
		true
	}

	/// Attempts to fill the bootstrap set with the given peers until it reaches
	/// capacity.
	///
	/// Returns `true` if any peer was inserted, or `false` if the set was not
	/// modified.
	pub fn try_populate(&mut self, peers: impl Iterator<Item = PeerId>) -> bool {
		let mut modified = false;
		for peer_id in peers {
			if self.is_full() {
				break;
			}

			if self.try_insert(peer_id) {
				modified = true;
			}
		}

		modified
	}

	/// Fetches the current bootstrap set from the DHT.
	///
	/// If there is no existing record returns an empty set.
	pub async fn fetch(client: &Client, network: &NetworkRecord) -> Self {
		let mut timestamp = None;
		let mut records = ArrayVec::new();

		if let Some(packet) = client.resolve_most_recent(network.public_key()).await
		{
			for (pos, record) in packet.all_resource_records().enumerate() {
				let Ok(parsed) = PeerRecord::parse(record, network).inspect_err(|e| {
					tracing::debug!(
						error = %e,
						position = pos,
						network = %network.network_id,
						"skipping malformed DHT peer record",
					);
				}) else {
					continue;
				};

				if records.try_push(parsed).is_err() {
					// seems like some node added more entries than the configured
					// maximum, skip the rest of the records.
					break;
				}
			}

			timestamp = Some(packet.timestamp());
		}

		Self { records, timestamp }
	}

	/// Publishes this bootstrap set to the DHT, replacing all existing records.
	pub async fn publish(
		&self,
		client: &Client,
		network: &NetworkRecord,
	) -> Result<(), PublishError> {
		let mut builder = SignedPacket::builder();
		for record in &self.records {
			builder = builder.txt(
				Name::new_unchecked(&record.encoded_name()),
				TXT::new(),
				PEER_RECORD_TTL.as_secs() as u32,
			);
		}

		let packet = builder.sign(network.keypair())?;
		client.publish(&packet, self.timestamp).await?;

		Ok(())
	}
}

#[derive(Debug, thiserror::Error)]
enum ParseError {
	#[error("Peer record is malformed: {0}")]
	Bs58DecodeError(#[from] b58::DecodeError),

	#[error("Peer TXT record has invalid network id suffix")]
	InvalidSuffix,

	#[error("PeerId has invalid length: expected 32 bytes, got {0}")]
	InvalidPeerIdLen(usize),

	#[error("PeerId is invalid: {0}")]
	InvalidPeerId(#[from] KeyParsingError),
}

#[derive(Debug, thiserror::Error)]
enum PublishError {
	#[error("Failed to build DHT record packet: {0}")]
	BuildError(#[from] SignedPacketBuildError),

	#[error("Failed to publish to DHT: {0}")]
	PublishError(#[from] pkarr::errors::PublishError),
}
