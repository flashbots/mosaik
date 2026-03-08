use {
	crate::cli::CliOpts,
	core::convert::Infallible,
	iroh::SecretKey,
	itertools::Itertools,
	mosaik::{Digest, NetworkId, PeerId, Tag, *},
};

#[derive(Debug, clap::Parser)]
pub struct BeaconCommand {
	/// Network ID to query
	#[clap(short, long)]
	pub network: NetworkId,

	/// The secret key to use for the bootstrap node. Ensures stable peer id.
	#[clap(
		short, 
		long, 
		value_parser = parse_secret_key,
	)]
	secret: Option<SecretKey>,

	/// Other bootstrap nodes to connect to on startup.
	#[clap(short, long)]
	peers: Vec<PeerId>,

	/// Tags to associate with the bootstrap node.
	#[clap(short, long, default_value = "beacon-node")]
	tags: Vec<Tag>,

	/// Do not use relay servers for this node.
	///
	/// Disables listening or dialing relays. Make sure that this node is
	/// directly reachable by other nodes without any NAT if you set this option.
	#[clap(long, default_value_t = false)]
	no_relay: bool,
}

impl BeaconCommand {
	pub async fn run(&self, _: &CliOpts) -> anyhow::Result<()> {
		println!("Starting beacon node with the following configuration:");
		println!("  network: {:?}", self.network);

		let secret = self
			.secret
			.clone()
			.unwrap_or_else(|| SecretKey::generate(&mut rand::rng()));

		let mut builder = Network::builder(self.network)
			.with_secret_key(secret)
			.with_discovery(
				discovery::Config::builder()
					.with_tags(self.tags.clone())
					.with_bootstrap(self.peers.clone()),
			);

		if self.no_relay {
			builder = builder.with_relay_mode(iroh::RelayMode::Disabled);
		}

		let network = builder.build().await?;

		println!("  peer id: {}", network.local().id());
		println!(
			"  tags   : {}",
			if self.tags.is_empty() {
				"none".to_string()
			} else {
				self.tags.iter().map(|t| format!("{t:?}")).join(", ")
			}
		);
		println!(
			"  peers  : {}",
			if self.peers.is_empty() {
				"none".to_string()
			} else {
				self.peers.iter().map(|p| p.to_string()).join(", ")
			}
		);
		println!(
			"  ips    : {}",
			network.local().addr().ip_addrs().join(", ")
		);
		println!(
			"  relays : {}",
			if self.no_relay {
				"disabled".to_string()
			} else {
				network.local().addr().relay_urls().join(", ")
			}
		);
		println!();
		println!("Hit Ctrl+C to stop the node.");

		// Keep the node running indefinitely.
		futures::future::pending::<()>().await;
		Ok(())
	}
}

#[expect(
	clippy::unnecessary_wraps,
	reason = "clap parser requires this signature"
)]
fn parse_secret_key(s: &str) -> Result<SecretKey, Infallible> {
	let bytes = Digest::from(s);
	Ok(SecretKey::from_bytes(bytes.as_bytes()))
}
