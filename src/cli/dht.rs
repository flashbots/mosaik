use {
	crate::cli::CliOpts,
	core::{net::SocketAddr, str::FromStr, time::Duration},
	futures::{SinkExt, StreamExt},
	iroh::{Endpoint, EndpointAddr, RelayUrl},
	mosaik::{NetworkId, PeerId, discovery::SignedPeerEntry},
	pkarr::{
		Client,
		Keypair,
		dns::{ResourceRecord, rdata::RData},
	},
	tokio::time::timeout,
	tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec},
};

#[derive(Debug, clap::Parser)]
pub struct DhtCommand {
	/// Network ID to query
	#[clap(short, long)]
	pub network: NetworkId,

	/// Check the status of the bootstrap peer by trying to connect to it.
	#[clap(long, default_value = "true")]
	pub check_status: bool,

	/// Timeout in seconds for the status check connection attempt.
	#[clap(short, long, default_value = "10")]
	pub timeout: u64,

	/// Show full information about the bootstrap peer.
	#[clap(short, long)]
	pub full_info: bool,
}

impl DhtCommand {
	pub async fn run(&self, _: &CliOpts) -> anyhow::Result<()> {
		let client = Client::builder().no_relays().cache_size(0).build()?;
		let keypair = Keypair::from_secret_key(self.network.as_bytes());
		let public_key = keypair.public_key();
		let Some(packet) = client.resolve_most_recent(&public_key).await else {
			anyhow::bail!("Network has no bootstrap record in the Mainline DHT");
		};

		let peer_id_record =
			packet.resource_records("_id").next().ok_or_else(|| {
				anyhow::anyhow!("network has invalid DHT record, missing _id field")
			})?;

		let Some(peer_id) = parse_peer_id(peer_id_record) else {
			anyhow::bail!("network has invalid DHT record, missing valid peer id")
		};

		println!("bootstrap peer of network id {:?}:", self.network);
		println!("  peer id: {peer_id}");

		let mut addrs = Vec::new();
		for record in packet.resource_records("_ip") {
			if let Some(ip) = parse_ip(record) {
				println!("  address: {ip}");
				addrs.push(iroh::TransportAddr::Ip(ip));
			}
		}

		for record in packet.resource_records("_r") {
			if let Some(relay) = parse_relay(record) {
				println!("  relay: {relay}");
				addrs.push(iroh::TransportAddr::Relay(relay));
			}
		}

		println!("  timestamp: {}", packet.timestamp().format_http_date());

		if self.check_status {
			let endpoint = EndpointAddr::new(peer_id).with_addrs(addrs);
			match check_status(endpoint, self.timeout).await {
				Ok(peer_entry) => {
					println!("  status: healthy");

					if self.full_info {
						println!("  -----");
						println!(
							"  full discovery info: {:?}",
							mosaik::primitives::Pretty(&peer_entry)
						);
					}
				}
				Err(e) => {
					println!("  status: unhealthy ({e})");
				}
			}
		}

		Ok(())
	}
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
	addr: EndpointAddr,
	timeout_secs: u64,
) -> Result<SignedPeerEntry, anyhow::Error> {
	let local = Endpoint::builder().bind().await?;
	let connection = timeout(
		Duration::from_secs(timeout_secs),
		local.connect(addr, b"/mosaik/discovery/ping/1.0"),
	)
	.await
	.map_err(|_| anyhow::anyhow!("failed to connect to bootstrap peer: timeout"))?
	.map_err(|e| anyhow::anyhow!("failed to connect to bootstrap peer: {e}"))?;

	let (tx, rx) = connection.open_bi().await?;

	let mut sender = FramedWrite::new(tx, LengthDelimitedCodec::new());
	let mut receiver = FramedRead::new(rx, LengthDelimitedCodec::new());

	// send an empty ping request and wait for a peer entry response
	timeout(
		Duration::from_secs(timeout_secs),
		sender.send(postcard::to_allocvec(&())?.into()),
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

	Ok(postcard::from_bytes(&response)?)
}
