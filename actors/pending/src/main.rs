//! An example of nodes responsible for maintaining a pool of pending
//! transactions and bundles that made it through initial validation but have
//! not yet been included in a block.
//!
//! Notes:
//! - Sharded
//! - Priority Queues
//! - Accepts async update to orders

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
