//! An example demonstrating a node that maintains the canonical chain state and
//! updates the p2p network with the latest updates to the chain state.
//!
//! Examples of subscribers to data produced by this node include:
//! - RPC nodes that keep track of updates to signer nonces to filter out stale
//!   transactions.

use {
	clap::Parser,
	core::net::{Ipv4Addr, SocketAddrV4},
	engine_api::ChainUpdate,
	mosaik::prelude::*,
	primitives::SignerNonces,
	rblib::prelude::*,
};

mod cli;
mod primitives;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
	let opts = cli::Opts::parse();
	println!("Starting canon actor with options: {opts:#?}");

	let id = Identity::default();
	let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 40150).into();
	let network = Network::<Tcp>::new(id, addr).await?;

	if opts.optimism {
		println!("Running canonical chain state actor in optimism mode");
		run::<Optimism, _>(&network);
	} else {
		println!("Running canonical chain state actor in standard mode");
		run::<Ethereum, _>(&network);
	}

	Ok(())
}

fn run<P: Platform, T: Transport>(network: &Network<T>) {
	let chain_updates_rx = network.stream::<ChainUpdate<P>>();
	let nonces_tx = network.sink::<SignerNonces>();
}
