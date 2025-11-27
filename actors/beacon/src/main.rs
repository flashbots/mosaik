use {
	core::future::pending,
	mosaik::prelude::{iroh::Endpoint, *},
	shared::{anyhow, cli::CliNetOpts, tracing::info},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	let opts = CliNetOpts::default();
	info!("Starting beacon actor with options: {opts:#?}");

	let mut builder = Network::builder(opts.network_id);
	builder.with_bootstrap_peers(opts.bootstrap);

	if let Some(secret_key) = opts.secret_key {
		builder = builder.with_secret_key(secret_key);
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
