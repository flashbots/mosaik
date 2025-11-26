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
	mosaik::prelude::*,
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
	let network = Network::new(opts.network.network_id).await?;

	// wait for network to be online
	network.local().online().await;

	for bootstrap in opts.network.bootstrap {
		info!("Dialing bootstrap peer {bootstrap}");
		network.discovery().dial(bootstrap.into()).await?;
	}

	// consumers
	let nonces = network.consume::<NoncesUpdate>();
	let balances = network.consume::<BalancesUpdate>();

	// correlate streams by key
	let mut nonces = nonces.keyed_by(|update| update.block);
	let mut balances = balances.keyed_by(|update| update.block);

	info!("RPC server started on network {}", network.network_id());
	info!("nonces stream_id: {}", nonces.stream_id());
	info!("balances stream_id: {}", balances.stream_id());
	info!("rpc listen addr: {}", opts.listen_addr);

	nonces.status().subscribed().await;
	balances.status().subscribed().await;

	loop {
		tokio::select! {
			Some(nonces_update) = nonces.next() => {
				info!("Received nonces update for block {}", nonces_update.block);
			}
			Some(balances_update) = balances.next() => {
				info!("Received balances update for block {}", balances_update.block);
			}
		}
	}
}
