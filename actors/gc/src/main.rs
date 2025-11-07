//! An example of a node that is responsible for garbage collecting expired and
//! stale transactions from the pending pool and other caches.

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
