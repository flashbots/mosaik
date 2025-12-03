//! An example demonstrating an implementation of a public RPC endpoint that
//! accepts transactions and bundles from external clients and onboards them
//! into an internal order pool network.
//!
//! A public facing RPC endpoint has the following design considerations:
//!
//! - It should be cheap to instantiate and dispose of. This is the first line
//!   of defense against DDoS attacks and other malicious behavior from external
//!   clients. It should be relatively stateless and have fast startup times so
//!   it can be scaled up and down quickly.
//!
//! - It accepts transactions and bundles in their raw format and performs
//!   initial validation before they are forwarded to the internal peer-to-peer
//!   network.
//!
//! - It filters out transactions that have invalid nonces or insufficient funds
//!   based on the local state it maintains from the internal network.

use {
	mosaik::*,
	rblib::prelude::*,
	shared::model::{BalancesUpdate, NoncesUpdate},
	tracing::info,
};

mod cli;
// mod rpc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	let opts = cli::Opts::default();
	info!("Starting RPC actor with options: {opts:#?}");

	if opts.network.optimism {
		run::<Optimism>(opts).await
	} else {
		run::<Ethereum>(opts).await
	}
}

async fn run<P: Platform>(opts: cli::Opts) -> anyhow::Result<()> {
	// connect to mosaik network
	let network = Network::builder(opts.network.network_id)
		.with_discovery(
			discovery::Config::builder() //
				.with_bootstrap(opts.network.bootstrap),
		)
		.build()
		.await?;

	info!("local peer info: {:#?}", network.discovery().me());

	// consumers
	let nonces = network.streams().consume::<NoncesUpdate>();
	let balances = network.streams().consume::<BalancesUpdate>();

	Ok(())

	// // correlate streams by key
	// let nonces = nonces.keyed_by(|update| update.block);
	// let balances = balances.keyed_by(|update| update.block);

	// info!("RPC server started on network {}", network.network_id());
	// info!("nonces stream_id: {}", nonces.stream_id());
	// info!("balances stream_id: {}", balances.stream_id());
	// info!("rpc listen addr: {}", opts.listen_addr);

	// nonces.status().subscribed().await;
	// balances.status().subscribed().await;

	// let mut joined = Join::new(nonces, balances);

	// loop {
	// 	tokio::select! {
	// 		Some(KeyedDatum(block, (nonces, balances))) = joined.next() => {
	// 			info!("Received correlated updates at block {block}");
	// 			info!("  NoncesUpdate: {nonces:?}");
	// 			info!("  BalancesUpdate: {balances:?}");
	// 		}
	// 	}
	// }
}
