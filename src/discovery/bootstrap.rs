use {
	super::Config,
	crate::{
		NetworkId,
		PeerId,
		discovery::Catalog,
		network::LocalNode,
		primitives::{Short, UnboundedChannel},
	},
	core::time::Duration,
	pkarr::{
		Client,
		Keypair,
		PublicKey,
		SignedPacket,
		Timestamp,
		dns::{
			Name,
			ResourceRecord,
			rdata::{RData, TXT},
		},
	},
	rand::prelude::SliceRandom,
	std::time::Instant,
	tokio::{
		sync::{
			mpsc::{UnboundedReceiver, UnboundedSender},
			watch,
		},
		time::sleep,
	},
};

/// Maximum number of peer entries to maintain in a single DHT bootstrap record.
///
/// Individual Mainline DHT records are capped at 1000 bytes total (including
/// DNS headers, all resource records, and pkarr framing). Each peer entry is a
/// TXT record whose name is the base58-encoded peer ID (~44 chars), with DNS
/// label encoding and type/class/TTL/rdlength overhead adding up to ~58–100
/// bytes per entry.
///
/// A cap of 10 leaves sufficient headroom (~200 bytes) below the 1000-byte
/// limit to avoid encoding-related edge cases, while still providing more than
/// enough peers for any node to bootstrap from.
const MAX_BOOTSTRAP_PEERS: usize = 10;

/// Maximum number of retries when a CAS (compare-and-swap) conflict occurs
/// during DHT publish. This prevents infinite retry loops when many nodes are
/// racing to update the same record.
const MAX_CAS_RETRIES: usize = 10;

/// Poll interval used when the local catalog has no known peers.
///
/// Once at least one peer appears in the catalog, the poll interval relaxes to
/// the configured [`Config::dht_poll_interval`]. If peers later disconnect and
/// the catalog empties again, polling switches back to this aggressive rate.
const AGGRESSIVE_POLL_INTERVAL: Duration = Duration::from_secs(5);

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
}

impl DhtBootstrap {
	pub(super) fn new(
		local: &LocalNode,
		config: &Config,
		catalog: watch::Receiver<Catalog>,
	) -> Self {
		let events = UnboundedChannel::default();

		let publish_interval = config.dht_publish_interval;
		let poll_interval = config.dht_poll_interval;

		if publish_interval.is_some() || poll_interval.is_some() {
			let events_tx = events.sender().clone();

			let worker = WorkerLoop {
				events_tx,
				catalog,
				publish_interval,
				poll_interval,
				local_id: local.id(),
				network_id: *local.network_id(),
			};

			let cancel = local.termination().child_token();
			tokio::spawn(cancel.run_until_cancelled_owned(worker.run()));
		}

		Self { events }
	}

	pub const fn events(&mut self) -> &mut UnboundedReceiver<PeerId> {
		self.events.receiver()
	}
}

struct WorkerLoop {
	publish_interval: Option<Duration>,
	poll_interval: Option<Duration>,
	local_id: PeerId,
	network_id: NetworkId,
	events_tx: UnboundedSender<PeerId>,
	catalog: watch::Receiver<Catalog>,
}

/// Result of resolving the current DHT record and building an updated packet.
struct ResolvedRecord {
	packet: SignedPacket,
	cas: Option<Timestamp>,
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
	async fn run(self) {
		let local_id_encoded = b58::encode(self.local_id.as_bytes());
		let local_record_name = Name::new_unchecked(&local_id_encoded);
		let keypair = Keypair::from_secret_key(self.network_id.as_bytes());
		let public_key = keypair.public_key();
		let network_suffix = format!(".{public_key}");

		let Some(client) = self.init_client() else {
			return;
		};

		let mut publish_tick = self
			.publish_interval
			.map(|d| tokio::time::interval(with_jitter(d)));

		// Stagger the initial publish across nodes to reduce CAS contention
		// when many nodes start simultaneously.
		let startup_jitter = Duration::from_millis(rand::random_range(0..3000));
		sleep(startup_jitter).await;

		// Initial publish before entering the loop, so we don't have to wait for
		// the first publish interval to elapse before announcing ourselves to other
		// nodes.
		self
			.publish_cycle(
				&client,
				&keypair,
				&public_key,
				&local_record_name,
				&network_suffix,
				true, // no_retries: skip retries on startup to publish faster
			)
			.await;

		// Initial poll to discover any existing peers before the first poll
		// interval elapses.
		self
			.poll_cycle(&client, &public_key, &local_record_name, &network_suffix)
			.await;

		loop {
			let poll_delay = self.next_poll_delay();

			tokio::select! {
				() = tick(&mut publish_tick) => {
					self.publish_cycle(
						&client,
						&keypair,
						&public_key,
						&local_record_name,
						&network_suffix,
						false
					).await;
				}
				() = sleep_or_pending(poll_delay) => {
					self.poll_cycle(
						&client,
						&public_key,
						&local_record_name,
						&network_suffix,
					).await;
				}
			}
		}
	}

	/// Initializes the pkarr DHT client. Returns `None` if initialization
	/// fails, in which case the bootstrap mechanism is silently disabled.
	fn init_client(&self) -> Option<Client> {
		match Client::builder().no_relays().build() {
			Ok(client) => Some(client),
			Err(err) => {
				tracing::warn!(
					error = %err,
					network = %self.network_id,
					"failed to initialize DHT bootstrap client"
				);

				None
			}
		}
	}

	/// Returns the delay until the next poll cycle. Uses aggressive polling
	/// when the catalog has no known peers, otherwise falls back to the
	/// configured poll interval.
	fn next_poll_delay(&self) -> Option<Duration> {
		self.poll_interval?;

		if self.catalog.borrow().peers_count() == 0 {
			Some(AGGRESSIVE_POLL_INTERVAL)
		} else {
			self.poll_interval
		}
	}

	// ---- Poll cycle --------------------------------------------------------

	/// Resolves the current DHT record and emits discovery events for any
	/// peers found.
	async fn poll_cycle(
		&self,
		client: &Client,
		public_key: &PublicKey,
		local_record_name: &Name<'_>,
		network_suffix: &str,
	) {
		let Some(packet) = client.resolve_most_recent(public_key).await else {
			return;
		};

		self.observe_existing_peers(&packet, local_record_name, network_suffix);
	}

	/// Feeds the currently observed peers in the resolved record into the
	/// discovery events, so they can be added to the catalog if not already
	/// present.
	fn observe_existing_peers(
		&self,
		packet: &SignedPacket,
		local_record_name: &Name<'_>,
		network_suffix: &str,
	) {
		for record in packet.all_resource_records() {
			if record.name != *local_record_name {
				self.observe_peer_record(&record.name.to_string(), network_suffix);
			}
		}
	}

	// ---- Publish cycle -----------------------------------------------------

	/// Resolves the current DHT record, merges in the local peer's entry,
	/// and publishes the updated record. Retries on CAS conflicts up to
	/// [`MAX_CAS_RETRIES`] times.
	async fn publish_cycle(
		&self,
		client: &Client,
		keypair: &Keypair,
		public_key: &PublicKey,
		local_record_name: &Name<'_>,
		network_suffix: &str,
		no_retries: bool,
	) {
		let retries = if no_retries { 0 } else { MAX_CAS_RETRIES };

		for attempt in 0..=retries {
			let existing = client.resolve_most_recent(public_key).await;

			if let Some(ref packet) = existing {
				self.observe_existing_peers(packet, local_record_name, network_suffix);
			}

			let resolved = match self.build_record(
				existing.as_ref(),
				keypair,
				local_record_name,
				network_suffix,
			) {
				Ok(resolved) => resolved,
				Err(err) => {
					tracing::warn!(
						error = %err,
						network = %self.network_id,
						"failed to build DHT bootstrap record"
					);
					return;
				}
			};

			let publish_start = Instant::now();
			match client.publish(&resolved.packet, resolved.cas).await {
				Ok(()) => {
					tracing::trace!(
						network = %self.network_id,
						duration = ?publish_start.elapsed(),
						"published DHT bootstrap record"
					);
					return;
				}
				Err(err) => {
					if matches!(err, pkarr::errors::PublishError::Concurrency(_)) {
						tracing::trace!(
							attempt,
							network = %self.network_id,
							"CAS conflict, retrying DHT bootstrap publish"
						);
						sleep(with_jitter(AGGRESSIVE_POLL_INTERVAL)).await;
						continue;
					}

					tracing::trace!(
						error = %err,
						network = %self.network_id,
						"failed to publish DHT bootstrap record"
					);
					return;
				}
			}
		}
	}

	/// Builds an updated signed packet that includes the local peer.
	/// Applies capacity management: if the record already contains
	/// [`MAX_BOOTSTRAP_PEERS`] entries, one existing entry is randomly
	/// evicted to make room for the local peer.
	fn build_record(
		&self,
		existing: Option<&SignedPacket>,
		keypair: &Keypair,
		local_record_name: &Name<'_>,
		network_suffix: &str,
	) -> Result<ResolvedRecord, pkarr::errors::SignedPacketBuildError> {
		let ttl_secs = self
			.publish_interval
			.expect("publish_cycle called without publish_interval")
			.as_secs() as u32;
		let cas;

		let mut builder = SignedPacket::builder();

		if let Some(most_recent) = existing {
			cas = Some(most_recent.timestamp());

			// Collect existing peer records (excluding our own).
			let mut peer_records: Vec<&ResourceRecord<'_>> = most_recent
				.all_resource_records()
				.filter(|r| r.name != *local_record_name)
				.collect();

			// Apply capacity management: if the record is full, evict the
			// least-connected peer (lowest self-reported catalog size) to
			// make room for ourselves, and dial into it so it doesn't get
			// stranded.
			if peer_records.len() >= MAX_BOOTSTRAP_PEERS {
				// Shuffle first so that peers with equal counts are evicted
				// randomly rather than always picking the same victim in
				// the (common) case where many peers report the same count.
				peer_records.shuffle(&mut rand::rng());
				peer_records.sort_by_key(|r| peer_count_from_record(r));
				let evicted = peer_records.remove(0);

				tracing::trace!(
					evicted = %evicted.name,
					peer_count = peer_count_from_record(evicted),
					total = peer_records.len() + 1,
					network = %self.network_id,
					"DHT bootstrap record at capacity, evicting least-connected peer"
				);

				// Dial into the evicted peer so it can still join the network
				// even though it's been removed from the shared record.
				let evicted_name = evicted.name.to_string();
				if let Some(peer_id) = parse_peer_id(&evicted_name, network_suffix)
					.filter(|id| *id != self.local_id)
				{
					let _ = self.events_tx.send(peer_id);
				}
			}

			for record in peer_records {
				builder = builder.record(record.clone());
			}
		} else {
			cas = None;
		}

		// Always insert our own entry with the current catalog peer count
		// so other nodes know how well-connected we are.
		let peers_attr = format!("peers={}", self.catalog.borrow().peers_count());
		let txt = TXT::new()
			.with_string(&peers_attr)
			.expect("peer count fits in TXT character string");

		let packet = builder
			.txt(local_record_name.clone(), txt, ttl_secs)
			.sign(keypair)?;

		Ok(ResolvedRecord { packet, cas })
	}

	/// Attempts to decode a peer ID from a DNS record name and emits a
	/// discovery event if successful. Silently ignores malformed records and
	/// the local peer's own record.
	fn observe_peer_record(&self, record_name: &str, network_suffix: &str) {
		let Some(peer_id) = parse_peer_id(record_name, network_suffix) else {
			return;
		};

		if peer_id == self.local_id {
			return;
		}

		tracing::trace!(
			peer_id = %Short(&peer_id),
			network = %self.network_id,
			"discovered peer via DHT bootstrap"
		);
		let _ = self.events_tx.send(peer_id);
	}
}

/// Waits for the next tick of an optional interval. If the interval is
/// `None`, the returned future never completes.
async fn tick(interval: &mut Option<tokio::time::Interval>) {
	match interval.as_mut() {
		Some(i) => {
			i.tick().await;
		}
		None => std::future::pending().await,
	}
}

/// Sleeps for the given duration, or pends forever if `None`.
async fn sleep_or_pending(delay: Option<Duration>) {
	match delay {
		Some(d) => tokio::time::sleep(d).await,
		None => std::future::pending().await,
	}
}

/// Adds random jitter (up to 1/3 of the base duration) to desynchronize
/// concurrent publishers.
fn with_jitter(base: Duration) -> Duration {
	let max_jitter_secs = base.as_secs() / 3 + 1;
	let jitter = Duration::from_secs(rand::random_range(0..max_jitter_secs));
	base + jitter
}

/// Extracts the self-reported peer count from a TXT resource record.
///
/// Expects the TXT data to contain a `peers=N` attribute. Returns `0` if the
/// record has no TXT data or the attribute is missing/malformed (which also
/// covers records written by older versions without peer count info).
fn peer_count_from_record(record: &ResourceRecord<'_>) -> usize {
	if let RData::TXT(ref txt) = record.rdata {
		txt
			.attributes()
			.get("peers")
			.and_then(|v| v.as_ref())
			.and_then(|s| s.parse().ok())
			.unwrap_or(0)
	} else {
		0
	}
}

/// Parses a peer ID from a DNS record name of the form
/// `<base58-peer-id>.<network-public-key>`.
///
/// Returns `None` if the name doesn't match the expected format or contains
/// an invalid peer ID.
fn parse_peer_id(record_name: &str, network_suffix: &str) -> Option<PeerId> {
	let peer_id_str = record_name.strip_suffix(network_suffix)?;
	let peer_id_bytes = b58::decode(peer_id_str).ok()?;
	let peer_id_bytes: [u8; 32] = peer_id_bytes.try_into().ok()?;
	PeerId::from_bytes(&peer_id_bytes).ok()
}
