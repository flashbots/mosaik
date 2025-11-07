//! Engine API actor
//!
//! This actor handles Mosaik's network interface to the consensus layer's
//! engine API.
//!
//! It is responsible for:
//!
//! - Receiving updates about new canonical chain updates though the engine API
//!   and propagating them to other actors.
//!
//! - Receiving requests for new payloads from the consensus client and
//!   propagating them to the rest of the network.
//!
//! - Propagating new payloads built by the network to the consensus client
//!   through the engine API.

use {
	clap::Parser,
	core::net::{Ipv4Addr, SocketAddrV4},
	engine_api::{ChainUpdate, PayloadJob, PayloadProposal},
	mosaik::prelude::*,
	rblib::prelude::*,
};

mod cli;
mod primitives;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
	let opts = cli::Opts::parse();
	println!("Starting engine-api actor with options: {opts:#?}");

	let n = std::any::type_name::<PayloadJob<Ethereum>>();
	println!("PayloadJob type: {n}");

	let id = Identity::default();
	let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 40150).into();
	let network = Network::<Tcp>::new(id, addr).await?;

	if opts.optimism {
		println!("Running in optimism mode");
		run::<Optimism, _>(&network);
	} else {
		println!("Running in standard mode");
		run::<Ethereum, _>(&network);
	}

	Ok(())
}

fn run<P: Platform, T: Transport>(network: &Network<T>) {
	let jobs_tx = network.sink::<PayloadJob<P>>();
	let payloads_rx = network.stream::<PayloadProposal<P>>();
	let chain_updates_tx = network.sink::<ChainUpdate<P>>();
}
