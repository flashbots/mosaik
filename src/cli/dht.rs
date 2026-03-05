use {
	crate::cli::CliOpts,
	colored::Colorize,
	core::{net::SocketAddr, str::FromStr, time::Duration},
	futures::{SinkExt, StreamExt, future::join_all},
	iroh::{Endpoint, EndpointAddr, RelayUrl, TransportAddr},
	mosaik::{NetworkId, PeerId, discovery::SignedPeerEntry, primitives},
	pkarr::{
		Client,
		Keypair,
		dns::{ResourceRecord, rdata::RData},
	},
	tokio::time::timeout,
	tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec},
};

/// Number of DHT slots in the bootstrap chain (matches discovery module).
const CHAIN_DEPTH: usize = 16;

#[derive(Debug, clap::Parser)]
pub struct DhtCommand {
	/// Network ID to query
	#[clap(short, long)]
	pub network: NetworkId,

	/// Check the status of the bootstrap peers by trying to connect to them.
	#[clap(long, default_value = "true")]
	pub check_status: bool,

	/// Timeout in seconds for each status check connection attempt.
	#[clap(short, long, default_value = "10")]
	pub timeout: u64,
}

impl DhtCommand {
	pub async fn run(&self, _: &CliOpts) -> anyhow::Result<()> {
		let client = Client::builder().build()?;

		// Build the chain of public keys, mirroring the discovery module's
		// NetworkRecord::chain() logic: slot 0 is derived directly from the
		// network ID, each subsequent slot from blake3(previous_network_id).
		let chain = build_chain(self.network, CHAIN_DEPTH);

		// Resolve all slots in parallel.
		let fetches = chain.iter().enumerate().map(|(idx, public_key)| {
			let client = &client;
			async move { (idx, client.resolve_most_recent(public_key).await) }
		});

		let resolved: Vec<_> = join_all(fetches).await;

		println!("DHT bootstrap chain for network {:?}:", self.network);
		println!();

		let mut occupied = 0u32;
		let mut healthy = 0u32;
		let mut unhealthy = 0u32;

		let local_endpoint = if self.check_status {
			Some(Endpoint::builder().bind().await?)
		} else {
			None
		};

		for (idx, packet) in &resolved {
			let Some(packet) = packet else {
				println!("{idx:>2}: {}", "empty".dimmed());
				continue;
			};

			let Some(peer_id_record) = packet.resource_records("_id").next() else {
				println!("{idx:>2}: {}", "invalid (missing _id)".dimmed());
				continue;
			};

			let Some(peer_id) = parse_peer_id(peer_id_record) else {
				println!("{idx:>2}: {}", "invalid (malformed peer id)".dimmed());
				continue;
			};

			occupied += 1;

			let mut addrs = Vec::new();
			for record in packet.resource_records("_ip") {
				if let Some(ip) = parse_ip(record) {
					addrs.push(TransportAddr::Ip(ip));
				}
			}

			for record in packet.resource_records("_r") {
				if let Some(relay) = parse_relay(record) {
					addrs.push(TransportAddr::Relay(relay));
				}
			}

			println!("{idx:>2}: {}", peer_id.to_string().cyan());
			println!("      timestamp: {}", packet.timestamp().format_http_date());

			for addr in &addrs {
				match addr {
					TransportAddr::Ip(ip) => {
						println!("      address  : {ip}");
					}
					TransportAddr::Relay(relay) => {
						println!("      relay    : {relay}");
					}
					_ => {
						println!("      address  : {addr:?}");
					}
				}
			}

			if let Some(local) = &local_endpoint {
				let endpoint = EndpointAddr::new(peer_id).with_addrs(addrs);
				match check_status(local, endpoint, self.timeout).await {
					Ok(peer_entry) => {
						healthy += 1;
						println!(
							"      status   : {} (uptime: {})",
							"healthy".green(),
							humantime::format_duration(Duration::from_secs(
								peer_entry.uptime().as_secs()
							))
						);
					}
					Err(e) => {
						unhealthy += 1;
						println!("      status   : {} ({e})", "unhealthy".red());
					}
				}
			}
		}

		println!();
		println!("  summary: {occupied}/{CHAIN_DEPTH} slots occupied");
		if self.check_status {
			println!(
				"           {} healthy, {} unhealthy",
				healthy.to_string().green(),
				unhealthy.to_string().red()
			);
		}

		Ok(())
	}
}

/// Builds the chain of public keys for all DHT slots.
///
/// Mirrors the `NetworkRecord::chain()` logic from the discovery module:
/// slot 0 is derived directly from the network ID, and each subsequent slot
/// is derived by hashing the previous slot's network ID with blake3.
fn build_chain(network_id: NetworkId, depth: usize) -> Vec<pkarr::PublicKey> {
	let mut chain = Vec::with_capacity(depth);
	let mut current_id = network_id;
	for _ in 0..depth {
		let keypair = Keypair::from_secret_key(current_id.as_bytes());
		chain.push(keypair.public_key());
		current_id = current_id.derive(current_id);
	}
	chain
}

fn parse_peer_id(record: &ResourceRecord<'_>) -> Option<PeerId> {
	if let RData::TXT(record) = &record.rdata {
		PeerId::from_bytes(
			&b58::decode(record.attributes().iter().next()?.0)
				.ok()?
				.try_into()
				.ok()?,
		)
		.ok()
	} else {
		tracing::debug!(
			"bootstrap record in DHT has invalid format: missing TXT record for \
			 peer id"
		);
		None
	}
}

fn parse_ip(record: &ResourceRecord<'_>) -> Option<SocketAddr> {
	if let RData::TXT(ip) = &record.rdata {
		SocketAddr::from_str(ip.attributes().iter().next()?.0).ok()
	} else {
		None
	}
}

fn parse_relay(record: &ResourceRecord<'_>) -> Option<RelayUrl> {
	if let RData::TXT(relay) = &record.rdata {
		RelayUrl::from_str(relay.attributes().iter().next()?.0).ok()
	} else {
		None
	}
}

async fn check_status(
	local: &Endpoint,
	addr: EndpointAddr,
	timeout_secs: u64,
) -> Result<SignedPeerEntry, anyhow::Error> {
	let connection = timeout(
		Duration::from_secs(timeout_secs),
		local.connect(addr, b"/mosaik/discovery/ping/1.0"),
	)
	.await
	.map_err(|_| anyhow::anyhow!("timeout"))?
	.map_err(|e| anyhow::anyhow!("{e}"))?;

	let (tx, rx) = connection.open_bi().await?;

	let mut sender = FramedWrite::new(tx, LengthDelimitedCodec::new());
	let mut receiver = FramedRead::new(rx, LengthDelimitedCodec::new());

	// send an empty ping request and wait for a peer entry response
	timeout(
		Duration::from_secs(timeout_secs),
		sender.send(primitives::encoding::try_serialize(&()).unwrap()),
	)
	.await
	.map_err(|e| {
		anyhow::anyhow!("failed to send ping request to bootstrap peer: {e}")
	})??;

	let response = timeout(Duration::from_secs(timeout_secs), receiver.next())
		.await
		.map_err(|_| {
			anyhow::anyhow!(
				"failed to receive ping response from bootstrap peer: timeout"
			)
		})?
		.ok_or_else(|| {
			anyhow::anyhow!("peer did not respond to ping request within timeout")
		})??;

	Ok(primitives::encoding::deserialize(&response)?)
}
