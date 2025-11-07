//! An example of a node that calculates quality scores for transactions
//! asynchronously and updates the pending pool with the latest scores.

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
