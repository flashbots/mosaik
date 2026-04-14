use {
	super::Config,
	crate::{
		NetworkId,
		PeerId,
		primitives::{Short, UnboundedChannel},
	},
	core::{net::SocketAddr, str::FromStr, time::Duration},
	futures::future::join_all,
	iroh::{EndpointAddr, RelayUrl, TransportAddr},
	pkarr::{dns::rdata::RData, errors::SignedPacketBuildError, *},
	std::sync::Arc,
	tokio::{
		sync::mpsc::{UnboundedReceiver, UnboundedSender},
		time::{Instant, sleep},
	},
};

/// Number of DHT slots in the bootstrap chain.
///
/// Each slot is derived by recursively hashing the network ID, forming a
/// deterministic linked list of DHT keys. More slots means more resilience
/// to churn and less contention when many nodes boot simultaneously, at the
/// cost of more DHT lookups per poll cycle.
const CHAIN_DEPTH: usize = 16;

/// Interval for polling the DHT for peer records when the catalog has at least
/// one known peer.
const RELAXED_POLL_INTERVAL: Duration = Duration::from_secs(60);

/// Poll interval used when the local catalog has no known peers.
///
/// Once at least one peer appears in the catalog, the poll interval relaxes to
/// the configured [`Config::dht_poll_interval`]. If peers later disconnect and
/// the catalog empties again, polling switches back to this aggressive rate.
const AGGRESSIVE_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// TTL for a peer record in the DHT - 10 minutes.
const PEER_RECORD_TTL: Duration = Duration::from_secs(600);

/// Timeout for pinging a peer discovered in the DHT to check if it's healthy.
const PEER_PING_TIMEOUT: Duration = Duration::from_secs(5);

/// Automatic bootstrap mechanism for Mosaik networks.
///
/// Automatic bootstrap works by maintaining a chain of [`CHAIN_DEPTH`] DHT
/// slots, each holding one bootstrap peer's address. The slots form a
/// deterministic linked list: slot 0 is keyed by the network ID, and each
/// subsequent slot is keyed by `blake3(previous_slot_network_id)`.
///
/// On each poll cycle, all slots are resolved in parallel. Every healthy peer
/// found is fed into the discovery system. If the local node is not already
/// present in any slot, it claims the first empty or unhealthy slot so that
/// other nodes can discover it.
///
/// This reduces contention compared to a single-slot design and dramatically
/// speeds up first-peer discovery under high churn, since a new node only
/// needs *one* of the 16 slots to contain a healthy peer.
pub struct DhtBootstrap {
	updates: UnboundedChannel<EndpointAddr>,
}

impl DhtBootstrap {
	pub(super) fn new(handle: Arc<super::Handle>, config: &Config) -> Self {
		let updates = UnboundedChannel::default();

		if config.no_auto_bootstrap {
			tracing::debug!(
				network = %handle.local.network_id(),
				"DHT auto bootstrap is disabled"
			);
		} else {
			let updates_tx = updates.sender().clone();
			let worker = WorkerLoop {
				handle,
				updates_tx,
				last_published_at: None,
			};
			let cancel = worker.handle.local.termination().child_token();
			tokio::spawn(cancel.run_until_cancelled_owned(worker.run()));
		}

		Self { updates }
	}

	pub const fn updates(&mut self) -> &mut UnboundedReceiver<EndpointAddr> {
		self.updates.receiver()
	}
}

struct WorkerLoop {
	/// Handle to the main discovery worker loop.
	handle: Arc<super::Handle>,

	/// Channel for emitting discovered bootstrap peer addresses to the discovery
	/// system.
	updates_tx: UnboundedSender<EndpointAddr>,

	/// Tracks the last time this node successfully published to the DHT.
	///
	/// After publishing, the DHT entry may not be immediately visible to
	/// subsequent queries due to propagation latency across the Mainline
	/// network. Without this guard, the node would see itself as absent on
	/// the next scan and claim yet another slot, gradually filling the
	/// chain with copies of itself.
	last_published_at: Option<Instant>,
}

/// Result of scanning all DHT slots in the bootstrap chain.
struct ChainScanResult {
	/// Whether the local node already occupies a healthy slot in the chain.
	self_present: bool,

	/// All slots that are either empty or contain an unhealthy peer,
	/// any of which the local node could claim. Each entry contains the
	/// slot index and the CAS timestamp (if the slot was occupied).
	///
	/// A random candidate is chosen at publish time to spread nodes across
	/// the chain and reduce write contention when many nodes boot at once.
	publish_candidates: Vec<(usize, Option<Timestamp>)>,
}

impl ChainScanResult {
	/// Picks a random publish candidate from the available slots.
	///
	/// Randomizing the slot selection spreads concurrent publishers across
	/// different DHT keys, reducing CAS contention when many nodes boot
	/// simultaneously.
	fn random_publish_target(&self) -> Option<&(usize, Option<Timestamp>)> {
		if self.publish_candidates.is_empty() {
			return None;
		}
		let idx = rand::random_range(0..self.publish_candidates.len());
		Some(&self.publish_candidates[idx])
	}
}

impl WorkerLoop {
	/// Main loop: resolves all [`CHAIN_DEPTH`] DHT slots in parallel on each
	/// cycle, emits every healthy peer found, and claims the first available
	/// slot if the local node is not already present in the chain.
	///
	/// Starts with an aggressive poll interval when the catalog has no peers,
	/// and relaxes once connectivity is established. Automatically switches
	/// back to aggressive polling if all peers disconnect.
	async fn run(mut self) {
		// wait for the local node to be online and have its transport addresses
		// assigned and all protocols initialized.
		self.handle.local.online().await;

		let network = *self.handle.local.network_id();
		let Ok(client) = tokio::task::spawn_blocking(move || {
			Client::builder()
				.request_timeout(PEER_PING_TIMEOUT)
				.no_relays()
				.cache_size(0)
				.build()
		})
		.await
		.expect("DHT client builder task panicked")
		.inspect_err(|e| {
			tracing::warn!(
				error = %e,
				%network,
				"failed to initialize DHT bootstrap"
			);
		}) else {
			return;
		};

		let chain =
			NetworkRecord::chain(*self.handle.local.network_id(), CHAIN_DEPTH);

		loop {
			let scan = self.scan_chain(&client, &chain).await;

			let recently_published = self
				.last_published_at
				.is_some_and(|t| t.elapsed() < PEER_RECORD_TTL);

			if !scan.self_present
				&& !recently_published
				&& let Some(&(slot_idx, cas_timestamp)) = scan.random_publish_target()
			{
				match self
					.publish_self(&client, &chain[slot_idx], cas_timestamp)
					.await
				{
					Ok(()) => {
						self.last_published_at = Some(Instant::now());
						tracing::debug!(
							slot = slot_idx,
							peer_id = %Short(self.handle.local.id()),
							addrs = ?self.handle.local.addr().addrs,
							network = %Short(chain[0].network_id),
							"published local peer to Mainline DHT bootstrap record"
						);
					}
					Err(PublishError::PublishError(
						pkarr::errors::PublishError::Concurrency(_),
					)) => {
						tracing::trace!(
							network = %Short(chain[0].network_id),
							slot = slot_idx,
							"DHT publish race lost, retrying..."
						);
						continue;
					}
					Err(e) => {
						tracing::debug!(
							error = %e,
							network = %Short(chain[0].network_id),
							slot = slot_idx,
							"failed to publish to DHT bootstrap chain",
						);
					}
				}
			}

			sleep(self.next_poll_interval()).await;
		}
	}

	/// Resolves all slots in the bootstrap chain concurrently, pings each
	/// discovered peer, and determines whether the local node should publish
	/// itself into an available slot.
	async fn scan_chain(
		&mut self,
		client: &Client,
		chain: &[NetworkRecord],
	) -> ChainScanResult {
		let local_id = self.handle.local.id();
		let network_id = self.handle.local.network_id();
		let latest_local_addrs = self.handle.local.addr().clone();

		// Resolve all slots in parallel.
		let fetches = chain.iter().enumerate().map(|(idx, record)| async move {
			let result = Self::fetch(client, record).await.inspect(|(addr, _)| {
				tracing::trace!(
					slot = idx,
					peer_id = %Short(addr.id),
					addrs = ?addr.addrs,
					network = %Short(network_id),
					"fetched DHT record for bootstrap slot"
				);
			});
			(idx, result)
		});

		let resolved: Vec<_> = join_all(fetches).await;

		let mut self_present = false;
		let mut publish_candidates: Vec<(usize, Option<Timestamp>)> = Vec::new();
		let mut stale_self_slot: Option<(usize, Option<Timestamp>)> = None;

		for (idx, slot) in resolved {
			let Some((addr, timestamp)) = slot else {
				// Empty slot — candidate for publishing.
				publish_candidates.push((idx, None));
				continue;
			};

			if addr.id == local_id {
				// This slot already contains the local node.
				self_present = true;

				// Check if the addresses are stale and need refreshing.
				if addr.addrs != latest_local_addrs.addrs {
					tracing::trace!(
						network = %Short(network_id),
						slot = idx,
						peer_id = %Short(addr.id),
						"DHT slot has stale local addresses, will update"
					);
					// Record the exact stale slot so we update it
					// deterministically after the scan completes.
					stale_self_slot = Some((idx, Some(timestamp)));

					// Consider ourself not present and republish new addresses to
					// the same slot.
					self_present = false;

					// Clear the last published timestamp to allow immediate republishing
					// of updated addresses.
					self.last_published_at = None;
				}
				continue;
			}

			// Slot contains a different peer — ping it.
			if let Ok((peer_entry, rtt)) = self
				.handle
				.local
				.ping(addr.clone(), Some(PEER_PING_TIMEOUT))
				.await
			{
				// Record the RTT sample from the ping connection.
				if let Some(rtt) = rtt {
					self.handle.rtt.record_sample(addr.id, rtt);
				}

				// Healthy peer — emit it to the discovery system.
				if self.updates_tx.send(peer_entry.address().clone()).is_err() {
					tracing::warn!(
						network = %Short(network_id),
						"discovery channel closed, peer update lost"
					);
				}
			} else {
				// Unhealthy peer — candidate for replacement.
				tracing::trace!(
					slot = idx,
					peer_id = %Short(addr.id),
					network = %Short(network_id),
					"unhealthy peer in DHT slot, candidate for replacement"
				);
				publish_candidates.push((idx, Some(timestamp)));
			}
		}

		// If we found our own slot with stale addresses, target exactly
		// that slot rather than picking randomly from all candidates.
		if let Some(slot) = stale_self_slot {
			publish_candidates = vec![slot];
		}

		ChainScanResult {
			self_present,
			publish_candidates,
		}
	}

	/// Fetches and parses a single DHT slot's record. Returns the endpoint
	/// address and timestamp if the slot contains a valid record.
	async fn fetch(
		client: &Client,
		network: &NetworkRecord,
	) -> Option<(EndpointAddr, Timestamp)> {
		let packet = client.resolve_most_recent(network.public_key()).await?;

		let Some(peer_id_record) = packet.resource_records("_id").next() else {
			tracing::trace!(
				network = %Short(network.network_id),
				"DHT record has no _id resource record"
			);
			return None;
		};
		let peer_id = if let RData::TXT(record) = &peer_id_record.rdata {
			let attrs = record.attributes();
			let attr = attrs.iter().next()?.0;
			let decoded = b58::decode(attr)
				.inspect_err(|e| {
					tracing::trace!(
						network = %Short(network.network_id),
						error = %e,
						"failed to base58-decode peer ID from DHT record"
					);
				})
				.ok()?;
			let bytes: [u8; 32] = decoded
				.try_into()
				.inspect_err(|v: &Vec<u8>| {
					tracing::trace!(
						network = %Short(network.network_id),
						len = v.len(),
						"invalid peer ID byte length in DHT record (expected 32)"
					);
				})
				.ok()?;
			PeerId::from_bytes(&bytes)
				.inspect_err(|e| {
					tracing::trace!(
						network = %Short(network.network_id),
						error = %e,
						"failed to parse peer ID from DHT record"
					);
				})
				.ok()?
		} else {
			tracing::debug!(
				network = %Short(network.network_id),
				"bootstrap record in DHT has invalid format: missing TXT record for peer id"
			);
			return None;
		};

		let mut endpoint = EndpointAddr::new(peer_id);

		for record in packet.resource_records("_ip") {
			if let RData::TXT(ip) = &record.rdata {
				let attrs = ip.attributes();
				let Some(attr) = attrs.iter().next() else {
					continue;
				};
				match SocketAddr::from_str(attr.0) {
					Ok(addr) => {
						endpoint.addrs.insert(TransportAddr::Ip(addr));
					}
					Err(e) => {
						tracing::trace!(
							network = %Short(network.network_id),
							error = %e,
							"failed to parse IP address from DHT record"
						);
					}
				}
			}
		}

		for record in packet.resource_records("_r") {
			if let RData::TXT(relay) = &record.rdata {
				let attrs = relay.attributes();
				let Some(attr) = attrs.iter().next() else {
					continue;
				};
				match RelayUrl::from_str(attr.0) {
					Ok(url) => {
						endpoint.addrs.insert(TransportAddr::Relay(url));
					}
					Err(e) => {
						tracing::trace!(
							network = %Short(network.network_id),
							error = %e,
							"failed to parse relay URL from DHT record"
						);
					}
				}
			}
		}

		Some((endpoint, packet.timestamp()))
	}

	/// Publishes the local peer's address information to a specific DHT slot.
	///
	/// This is called when the local node is not yet present in the chain and
	/// an empty or unhealthy slot has been identified, or when the local node's
	/// slot has stale addresses that need refreshing.
	async fn publish_self(
		&self,
		client: &Client,
		network: &NetworkRecord,
		cas_timestamp: Option<Timestamp>,
	) -> Result<(), PublishError> {
		let addr = self.handle.local.addr();
		let mut packet = SignedPacket::builder().txt(
			"_id".try_into().unwrap(),
			b58::encode(addr.id.as_bytes()).as_str().try_into()?,
			PEER_RECORD_TTL.as_secs() as u32,
		);

		for addr in addr.addrs {
			match addr {
				TransportAddr::Ip(ip) => {
					packet = packet.txt(
						"_ip".try_into().unwrap(),
						ip.to_string().as_str().try_into()?,
						PEER_RECORD_TTL.as_secs() as u32,
					);
				}
				TransportAddr::Relay(relay_url) => {
					packet = packet.txt(
						"_r".try_into().unwrap(),
						relay_url.to_string().as_str().try_into()?,
						PEER_RECORD_TTL.as_secs() as u32,
					);
				}
				_ => {}
			}
		}

		let packet = packet.sign(network.keypair())?;
		Ok(client.publish(&packet, cas_timestamp).await?)
	}

	/// Determines the interval to wait before querying the DHT for bootstrap
	/// peers again.
	///
	/// When the current node is not connected to any peers on the gossip layer,
	/// it will poll the DHT at a more aggressive rate to quickly discover
	/// changes to the bootstrap peer record and connect to the first healthy peer
	/// as soon as it is published.
	fn next_poll_interval(&self) -> Duration {
		with_jitter(if self.handle.neighbors_count() == 0 {
			AGGRESSIVE_POLL_INTERVAL
		} else {
			RELAXED_POLL_INTERVAL
		})
	}
}

/// Adds random jitter (up to 1/3 of the base duration) to desynchronize
/// concurrent publishers.
fn with_jitter(base: Duration) -> Duration {
	let max_jitter_millis = base.as_millis() / 3 + 1;
	let jitter = rand::random_range(0..max_jitter_millis);
	let jitter = Duration::from_millis(jitter as u64);
	base + jitter
}

/// Represents one network's record in the DHT, which includes the network ID
/// and the keypair used to sign the record.
///
/// Multiple `NetworkRecord`s form a chain: slot 0 is derived directly from the
/// network ID, and each subsequent slot is derived by hashing the previous
/// slot's network ID with blake3. This creates a deterministic linked list of
/// DHT keys that any node can independently compute.
struct NetworkRecord {
	network_id: NetworkId,
	keypair: Keypair,
	public_key: PublicKey,
}

impl NetworkRecord {
	/// Creates a new `NetworkRecord` for the given network ID, deriving the
	/// keypair from the network ID bytes.
	pub fn new(network_id: NetworkId) -> Self {
		let keypair = Keypair::from_secret_key(network_id.as_bytes());
		let public_key = keypair.public_key();

		Self {
			network_id,
			keypair,
			public_key,
		}
	}

	/// Returns the next record in the chain by hashing this slot's network ID.
	///
	/// The next slot's network ID is `blake3(self.network_id)`, which makes the
	/// entire chain deterministic and computable by any node that knows the
	/// original network ID.
	pub fn next(&self) -> Self {
		Self::new(self.network_id.derive(self.network_id))
	}

	/// Builds the full bootstrap chain of [`CHAIN_DEPTH`] slots starting from
	/// the given network ID.
	pub fn chain(network_id: NetworkId, depth: usize) -> Vec<Self> {
		assert_ne!(depth, 0, "chain depth must be greater than 0");
		let mut chain = Vec::with_capacity(depth);
		chain.push(Self::new(network_id));
		for i in 1..depth {
			chain.push(chain[i - 1].next());
		}
		chain
	}

	/// Returns the public key corresponding to this network record, which is used
	/// as the DHT record key for bootstrapping this network.
	pub const fn public_key(&self) -> &PublicKey {
		&self.public_key
	}

	pub const fn keypair(&self) -> &Keypair {
		&self.keypair
	}
}

#[derive(Debug, thiserror::Error)]
enum PublishError {
	#[error("Failed to build DHT record packet: {0}")]
	BuildError(#[from] SignedPacketBuildError),

	#[error("Failed to publish to DHT: {0}")]
	PublishError(#[from] pkarr::errors::PublishError),

	#[error("DNS entry encoding error: {0}")]
	Encoding(#[from] dns::SimpleDnsError),
}
