use {
	super::Config,
	crate::{
		NetworkId,
		PeerId,
		discovery::Catalog,
		primitives::UnboundedChannel,
	},
	core::{net::SocketAddr, str::FromStr, time::Duration},
	iroh::{EndpointAddr, RelayUrl, TransportAddr},
	pkarr::{dns::rdata::RData, errors::SignedPacketBuildError, *},
	std::sync::Arc,
	tokio::{
		sync::{
			mpsc::{UnboundedReceiver, UnboundedSender},
			watch,
		},
		time::sleep,
	},
};

/// Interval for polling the DHT for peer records when the catalog has at least
/// one known peer.
const RELAXED_POLL_INTERVAL: Duration = Duration::from_secs(60);

/// Poll interval used when the local catalog has no known peers.
///
/// Once at least one peer appears in the catalog, the poll interval relaxes to
/// the configured [`Config::dht_poll_interval`]. If peers later disconnect and
/// the catalog empties again, polling switches back to this aggressive rate.
const AGGRESSIVE_POLL_INTERVAL: Duration = Duration::from_secs(5);

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
	events: UnboundedChannel<EndpointAddr>,
}

impl DhtBootstrap {
	pub(super) fn new(
		handle: Arc<super::Handle>,
		config: &Config,
		catalog: watch::Receiver<Catalog>,
	) -> Self {
		let events = UnboundedChannel::default();

		if config.no_auto_bootstrap {
			tracing::debug!(
				network = %handle.local.network_id(),
				"DHT auto bootstrap is disabled"
			);
		} else {
			let events_tx = events.sender().clone();

			let worker = WorkerLoop {
				handle,
				events_tx,
				catalog,
			};

			let cancel = worker.handle.local.termination().child_token();
			tokio::spawn(cancel.run_until_cancelled_owned(worker.run()));
		}

		Self { events }
	}

	pub const fn events(&mut self) -> &mut UnboundedReceiver<EndpointAddr> {
		self.events.receiver()
	}
}

struct WorkerLoop {
	handle: Arc<super::Handle>,
	events_tx: UnboundedSender<EndpointAddr>,
	catalog: watch::Receiver<Catalog>,
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
		let Ok(client) = Client::builder()
			.no_relays()
			.cache_size(0)
			.build()
			.inspect_err(|e| {
				tracing::warn!(
					error = %e,
					network = %self.handle.local.network_id(),
					"failed to initialize DHT bootstrap"
				);
			})
		else {
			return;
		};

		let network = NetworkRecord::new(*self.handle.local.network_id());

		loop {
			let mut publish_self = false;
			let mut cas_timestamp = None;
			let mut next_poll_relaxed =
				self.catalog.borrow().signed_peers_count() != 0;

			if let Some((addr, timestamp)) = self.fetch(&client, &network).await {
				tracing::trace!(
					network = %network.network_id,
					peer_id = %addr.id,
					addrs = ?addr.addrs,
					"fetched bootstrap peer record from DHT"
				);

				// The mainline DHT record for this network already exists
				if addr.id == self.handle.local.id() {
					// the DHT record refers to this local node, ensure that the record
					// has up to date addresses and update it if necessary with the latest
					// set of known transport addresses for this node.
					if addr.addrs != self.handle.local.addr().addrs {
						tracing::trace!(
							network = %network.network_id,
							peer_id = %addr.id,
							addrs = ?addr.addrs,
							"DHT record has stale addresses, updating"
						);
						publish_self = true;
						cas_timestamp = Some(timestamp);
					}
				} else if let Ok(peer_entry) =
					// the DHT record refers to a different peer, verify that it
					// is healthy and replace it with ourselves if its not.
					self.handle.local.ping(addr.clone(), None).await
				{
					// healthy bootstrap peer in mainline DHT, no need to replace it.
					// feed the entry to the rest of the discovery system.
					let _ = self.events_tx.send(peer_entry.address().clone());
				} else {
					// The bootstrap peer in the DHT is not responding to pings, consider
					// it unhealthy and replace it with ourselves.
					publish_self = true;
					cas_timestamp = Some(timestamp);

					tracing::trace!(
						network = %network.network_id,
						peer_id = %addr.id,
						addrs = ?addr.addrs,
						"DHT bootstrap peer unhealthy, replacing with local peer"
					);
				}
			} else {
				// no existing Mainline DHT record for this network-id, publish
				// ourselves as the bootstrap peer.
				publish_self = true;

				tracing::trace!(
					network = %network.network_id,
					"no DHT bootstrap record found, publishing local peer"
				);
			}

			if publish_self {
				// the current bootstrap peer is unhealthy or not present, replace
				// it with ourselves in the DHT so other nodes can discover us
				// instead.
				match self.publish_self(&client, &network, cas_timestamp).await {
					Ok(()) => {
						tracing::debug!(
							network = %network.network_id,
							peer_id = %self.handle.local.id(),
							addrs = ?self.handle.local.addr().addrs,
							"published local peer as bootstrap record to Mainline DHT"
						);
						next_poll_relaxed = true;
					}
					Err(PublishError::PublishError(
						pkarr::errors::PublishError::Concurrency(_),
					)) => {
						// lost the race to update the DHT record, retry,
						tracing::trace!(
							network = %network.network_id,
							"DHT publish race lost, retrying..."
						);
						continue;
					}
					Err(e) => {
						// unrecoverable error, skip this publish cycle and try again at
						// the next interval.
						tracing::warn!(
							error = %e,
							network = %network.network_id,
							"failed to overwrite unhealthy bootstrap peer in Mainline DHT",
						);
						next_poll_relaxed = false;
					}
				}
			}

			sleep(with_jitter(if next_poll_relaxed {
				RELAXED_POLL_INTERVAL
			} else {
				AGGRESSIVE_POLL_INTERVAL
			}))
			.await;
		}
	}

	/// Fetches the latest entry from the Mainline DHT for this network's public
	/// key and returns its parsed contents if valid.
	async fn fetch(
		&self,
		client: &Client,
		network: &NetworkRecord,
	) -> Option<(EndpointAddr, Timestamp)> {
		let packet = client.resolve_most_recent(network.public_key()).await?;

		let peer_id_record = packet.resource_records("_id").next()?;
		let peer_id = if let RData::TXT(record) = &peer_id_record.rdata {
			PeerId::from_bytes(
				&b58::decode(record.attributes().iter().next()?.0)
					.ok()?
					.try_into()
					.ok()?,
			)
			.ok()?
		} else {
			tracing::debug!(
				network = %network.network_id,
				"bootstrap record in DHT has invalid format: missing TXT record for peer id"
			);
			return None;
		};

		let mut endpoint = EndpointAddr::new(peer_id);

		for record in packet.resource_records("_ip") {
			if let RData::TXT(ip) = &record.rdata {
				endpoint.addrs.insert(TransportAddr::Ip(
					SocketAddr::from_str(ip.attributes().iter().next()?.0).ok()?,
				));
			}
		}

		for record in packet.resource_records("_r") {
			if let RData::TXT(relay) = &record.rdata {
				endpoint.addrs.insert(TransportAddr::Relay(
					RelayUrl::from_str(relay.attributes().iter().next()?.0).ok()?,
				));
			}
		}

		Some((endpoint, packet.timestamp()))
	}

	/// Publishes the given peer address information to the Mainline DHT as the
	/// bootstrap record for this network.
	///
	/// This is called for the local peer when the current bootstrap peer is
	/// deemed unhealthy, or when no bootstrap peer is present in the DHT.
	async fn publish_self(
		&self,
		client: &Client,
		network: &NetworkRecord,
		cas_timestamp: Option<Timestamp>,
	) -> Result<(), PublishError> {
		let addr = self.handle.local.addr();
		let mut packet = SignedPacket::builder().txt(
			"_id".try_into().unwrap(),
			b58::encode(addr.id.as_bytes()).as_str().try_into().unwrap(),
			PEER_RECORD_TTL.as_secs() as u32,
		);

		for addr in addr.addrs {
			match addr {
				TransportAddr::Ip(ip) => {
					packet = packet.txt(
						"_ip".try_into().unwrap(),
						ip.to_string().as_str().try_into().unwrap(),
						PEER_RECORD_TTL.as_secs() as u32,
					);
				}
				TransportAddr::Relay(relay_url) => {
					packet = packet.txt(
						"_r".try_into().unwrap(),
						relay_url.to_string().as_str().try_into().unwrap(),
						PEER_RECORD_TTL.as_secs() as u32,
					);
				}
				_ => {}
			}
		}

		let packet = packet.sign(network.keypair())?;
		Ok(client.publish(&packet, cas_timestamp).await?)
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
}
