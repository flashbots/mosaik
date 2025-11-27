use {
	core::future::pending,
	mosaik::prelude::*,
	shared::{
		anyhow,
		cli::CliNetOpts,
		tracing::{info, warn},
	},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	let opts = CliNetOpts::default();
	info!("Starting beacon actor with options: {opts:#?}");
	info!("v3");

	let mut builder =
		Network::builder(opts.network_id).with_bootstrap_peers(opts.bootstrap);

	if let Some(secret_key) = opts.secret_key {
		builder = builder.with_secret_key(secret_key);
	} else {
		warn!("Beacon actor started without a static public key!");
	}

	let network = builder.build().await?;

	// wait for network to be online
	network.local().online().await;

	info!(
		"Beacon actor started with identity {}",
		network.local().id(),
	);

	pending::<Result<(), _>>().await
}
