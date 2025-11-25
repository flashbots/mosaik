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
	crate::{
		rpc::RpcEndpoints,
		store::Store,
		sync::{BalancesUpdate, NoncesUpdate},
	},
	clap::Parser,
	mosaik::prelude::*,
	rblib::{alloy::consensus::Sealable, prelude::*},
	std::sync::Arc,
	tracing::info,
	tracing_subscriber::EnvFilter,
};

mod cli;
mod rpc;
mod store;
mod sync;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	tracing_subscriber::fmt()
		.with_env_filter(EnvFilter::from_default_env())
		.init();

	let opts = cli::Opts::parse();

	if opts.optimism {
		run::<Optimism>(opts).await
	} else {
		run::<Ethereum>(opts).await
	}
}

async fn run<P: Platform>(opts: cli::Opts) -> anyhow::Result<()> {
	// connect to mosaik network
	let network = Network::new(opts.network_id).await?;

	// local state store
	let store = Arc::new(Store::open(opts.data_dir)?);

	// consumers
	let nonces = network.consume::<NoncesUpdate>();
	let balances = network.consume::<BalancesUpdate>();
	let headers = network.consume::<types::Header<P>>();

	// correlate streams by key
	let mut nonces = nonces.keyed_by(|update| update.block);
	let mut balances = balances.keyed_by(|update| update.block);
	let mut headers = headers.keyed_by(|header| header.hash_slow());

	let inputs = nonces.join(balances).join(headers);

	// producers
	let mut bundles = network.produce::<types::Bundle<P>>();
	let mut transactions = network.produce::<types::Transaction<P>>();

	// RPC server
	let mut rpc = RpcEndpoints::<P>::start(
		store.clone(), //
		&opts.listen_addr,
	)
	.await?;

	info!("RPC server started on network {}", network.network_id());
	info!("nonces stream_id: {}", nonces.stream_id());
	info!("headers stream_id: {}", headers.stream_id());
	info!("balances stream_id: {}", balances.stream_id());
	info!("rpc listen addr: {}", opts.listen_addr);

	let mut last_header = ();
	let mut pending = Store::open(opts.data_dir.join("pending"))?;

	loop {
		tokio::select! {
			Some((nonces, balances, header)) = inputs.recv() => {
				if header.is_successor_of(&last_header) {
					// we're in sync, keep going
					store.merge(pending);
					store.insert_nonces(nonces).unwrap();
					store.insert_balances(balances).unwrap();
					last_header.set(header);
				} else {
					info!("We're out of sync, sync compacted history");
					pending.insert_nonces(nonces);
					pending.insert_balances(balances);
					// trigger full compacted stream sync for nonces and balances only
					todo!();
				}
			}
			// new validated transaction via RPC
			Some(tx) = rpc.transactions.recv() => {
				info!("New transaction received via RPC: {tx:?}");
				transactions.send(tx).await?;
			}

			// new validated bundle via RPC
			Some(bundle) = rpc.bundles.recv() => {
				info!("New bundle received via RPC: {bundle:?}");
				bundles.send(bundle).await?;
			}
		}
	}
}
