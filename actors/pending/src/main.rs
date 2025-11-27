//! An example of nodes responsible for maintaining a pool of pending
//! transactions and bundles that made it through initial validation but have
//! not yet been included in a block.
//!
//! Notes:
//! - Sharded
//! - Priority Queues
//! - Accepts async update to orders

use {
	mosaik::prelude::*,
	rblib::prelude::*,
	shared::{anyhow, cli::CliNetOpts, tracing::info},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	let opts = CliNetOpts::default();
	info!("Starting pending orders actor with options: {opts:#?}");

	if opts.optimism {
		run::<Optimism>(opts).await
	} else {
		run::<Ethereum>(opts).await
	}
}

async fn run<P: Platform>(opts: CliNetOpts) -> anyhow::Result<()> {
	let network = Network::builder(opts.network_id)
		.with_bootstrap_peers(opts.bootstrap)
		.build()
		.await?;

	// wait for network to be online
	network.local().online().await;

	info!(
		"Pending orders actor started with identity {}",
		network.local().id(),
	);

	let mut bundles_rx = network.consume::<types::Bundle<P>>();
	let mut transactions_rx = network.consume::<types::Transaction<P>>();

	Ok(())
}
