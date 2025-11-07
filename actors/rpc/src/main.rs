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

use {
	core::net::{Ipv4Addr, SocketAddrV4},
	jsonrpsee::{proc_macros::rpc, types::ErrorObjectOwned},
	mosaik::prelude::*,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
	let id = Identity::default();
	let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 40150).into();
	let network = Network::<Tcp>::new(id, addr).await?;

	let nonces = NonceStore;

	Ok(())
}

struct NonceStore;

#[rpc(server, namespace = "eth")]
pub trait EthApi {
	#[method(name = "sendRawTransaction")]
	async fn send_raw_transaction(
		&self,
		data: Vec<u8>,
	) -> Result<String, ErrorObjectOwned>;

	#[method(name = "sendBundle")]
	async fn send_bundle(
		&self,
		data: Vec<u8>,
	) -> Result<String, ErrorObjectOwned>;
}
