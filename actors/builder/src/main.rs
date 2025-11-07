//! An example demonstrating a block building node that is at the end of the
//! block building workflow and is responsible for assembling the final payload
//! of blocks that is handed off to the consensus layer.
//!
//! This is where an `rblib` pipeline would be instantiated to produce blocks
//! in response to FCU signals from the CL.
//!
//! Block payloads typically include at most about 200-300 transactions, so this
//! is the last-mile node that deals with an already filtered and condensed
//! set of transactions and bundles.

use {
	core::net::{Ipv4Addr, SocketAddrV4},
	mosaik::prelude::*,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
	let id = Identity::default();
	let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 40150).into();
	let network = Network::<Tcp>::new(id, addr).await?;

	Ok(())
}
