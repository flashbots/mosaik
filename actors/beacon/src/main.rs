use {
	core::future::pending,
	mosaik::prelude::{iroh::Endpoint, *},
	shared::{anyhow, cli::CliNetOpts, tracing::info},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	let opts = CliNetOpts::default();
	info!("Starting beacon actor with options: {opts:#?}");
	let endpoint = match &opts.secret_key {
		Some(sk) => Endpoint::builder().secret_key(sk.clone()).bind().await?,
		None => Endpoint::bind().await?,
	};

	let network = Network::with_endpoint(opts.network_id, endpoint).await?;

	// wait for network to be online
	network.local().online().await;

	info!(
		"Beacon actor started with identity {}",
		network.local().id(),
	);

	pending::<Result<(), _>>().await
}
