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
		state::{BalancesUpdate, NoncesUpdate, Store},
	},
	clap::Parser,
	mosaik::prelude::*,
	rblib::prelude::*,
	std::sync::Arc,
	tracing::info,
	tracing_subscriber::EnvFilter,
};

mod boot;
mod cli;
mod rpc;
mod state;

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

	// consumers
	let mut nonces = network.consume::<NoncesUpdate>();
	let mut balances = network.consume::<BalancesUpdate>();
	let mut headers = network.consume::<types::Header<P>>();

	// producers
	let mut bundles = network.produce::<types::Bundle<P>>();
	let mut transactions = network.produce::<types::Transaction<P>>();

	// local state store
	let store = Arc::new(Store::open(opts.data_dir)?);

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

	loop {
		tokio::select! {
			// stay in sync with the latest headers (todo)
			header = headers.recv() => {
				let header = header?;
				info!("New header: {header:?}");
				// todo: find out if we are in sync with the state
				// todo: of the chain, if not, sync up historical nonces
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

			// stay in sync with the latest nonces
			nonces = nonces.recv() => {
				let nonces = nonces?;
				info!("New nonces update: {nonces:?}");
				store.insert_nonces(nonces)?;
			}

			// stay in sync with the latest balances
			balances = balances.recv() => {
				let balances = balances?;
				info!("New balances update: {balances:?}");
				store.insert_balances(balances)?;
			}
		}
	}
}
