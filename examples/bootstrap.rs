//! # Bootstrap Node Example
//!
//! This example demonstrates how to build a bootstrap node for a mosaik
//! network.
//!
//! It can be used as-is for any network without modification.
//!
//! ## Configuration
//!
//! ### Secret Key
//!
//! Providing a secret key yields a stable peer ID, which is recommended for bootstrap nodes
//! so that other peers can reliably connect to them across restarts. The secret key can be
//! specified in one of two ways:
//!
//! - A 64-character hex string: will be used directly as the secret key.
//! - Any other string: will be treated as a seed and hashed into a secret key.
//!
//! ### Network ID
//!
//! The network ID can be specified in one of two ways:
//!
//! - A 64-character hex string: will be used directly as the network ID.
//! - Any other string: will be treated as a seed, and the network ID will be
//!   the hash of that string.
//!
//! ### Tags
//!
//! The node can be configured with a set of initial tags that will be
//! discoverable by other peers on the network.
//!
//! ### Dial Peers
//!
//! The node can be given a list of other peer IDs that it will attempt to
//! connect to on startup.
//!
//! ## Usage
//!
//! Other nodes on the network can use this node's public ID to get onboarded
//! onto the network and discover other nodes.
//! 
//! ```bash
//! bootstrap --network-id=net1 --secret=123
//! ```

use core::convert::Infallible;

use {
	clap::{ArgAction, Parser},
	mosaik::*,
};

/// Mosaik Network Bootstrap Node
#[derive(Debug, Parser)]
struct Opts {
	/// The secret key to use for the bootstrap node.
	#[clap(
		short, 
		long, 
		value_parser = parse_secret_key,
		env = "MOSAIK_BOOTSTRAP_SECRET"
	)]
	secret: Option<SecretKey>,

	/// The network id to use for the bootstrap node.
	#[clap(short, long, env = "MOSAIK_BOOTSTRAP_NETWORK_ID")]
	network_id: Option<NetworkId>,

	/// Other bootstrap nodes to connect to on startup.
	#[clap(short, long, env = "MOSAIK_BOOTSTRAP_PEERS")]
	peers: Vec<PeerId>,

	/// Tags to associate with the bootstrap node.
	#[clap(
		short,
		long,
		default_value = "bootstrap",
		env = "MOSAIK_BOOTSTRAP_TAGS"
	)]
	tags: Vec<Tag>,

	/// Do not use relay servers for this node.
	///
	/// Disables listening or dialing relays. Make sure that this node is
	/// directly reachable by other nodes without any NAT if you set this option.
	#[clap(long, default_value_t = false, env = "MOSAIK_BOOTSTRAP_NO_RELAY")]
	no_relay: bool,

	/// Use verbose output (-vv very verbose)
	#[clap(
		short,
		action = ArgAction::Count,
		env = "MOSAIK_BOOTSTRAP_LOGGING",
		help = "Use verbose output (-vv very verbose)",
		group = "logging"
	)]
	verbose: u8,

	/// Suppress all CLI output
	#[clap(short, long, env = "MOSAIK_BOOTSTRAP_QUIET", group = "logging")]
	quiet: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	let opts = Opts::parse();
	configure_logging(&opts);

	let secret = opts.secret.unwrap_or_else(|| {
		tracing::warn!("No secret key provided, generating random key");
		SecretKey::generate(&mut rand::rng())
	});

	let network_id = opts.network_id.unwrap_or_else(|| {
		tracing::warn!("No network id provided, generating random network id");
		NetworkId::random()
	});

	let mut builder = Network::builder(network_id)
		.with_secret_key(secret)
		.with_discovery(
			discovery::Config::builder()
				.with_tags(opts.tags.clone())
				.with_bootstrap(opts.peers.clone()),
		);

	if opts.no_relay {
		builder = builder.with_relay_mode(iroh::RelayMode::Disabled);
	}

	let network = builder.build().await?;

	tracing::info!("Bootstrap node started");
	tracing::info!("Public Id: {}", network.local().id());
	tracing::info!("Network Id: {:?}", network.network_id());
	tracing::info!("Tags: {:?}", opts.tags);
	tracing::info!("Bootstrap peers: {:?}", opts.peers);

	if opts.no_relay {
		tracing::info!("Relays: Disabled");
	} else {
		tracing::info!("Relays: Enabled");
	}

	tracing::info!("Hit Ctrl+C to stop the bootstrap node.");
	core::future::pending::<()>().await;

	Ok(())
}

fn configure_logging(opts: &Opts) {
	use {
		tracing::Level,
		tracing_subscriber::{
			Layer,
			filter::filter_fn,
			layer::SubscriberExt,
			util::SubscriberInitExt,
		},
	};

	if opts.quiet {
		return;
	}

	let log_level = match opts.verbose {
		1 => Level::DEBUG,
		2 => Level::TRACE,
		_ => Level::INFO,
	};

	tracing_subscriber::registry()
		.with(
			tracing_subscriber::fmt::layer()
				.with_filter(filter_fn(move |metadata| metadata.level() <= &log_level)),
		)
		.init();
}

#[expect(
	clippy::unnecessary_wraps, 
	reason = "clap parser requires this signature"
)]
fn parse_secret_key(s: &str) -> Result<SecretKey, Infallible> {
	let bytes = Digest::from(s);
	Ok(SecretKey::from_bytes(bytes.as_bytes()))
}
