use mosaik::{tee::tdx::NetworkTicketExt, *};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	println!("TDX image builder");
	// use tracing_subscriber::prelude::*;
	// tracing_subscriber::registry()
	// 	.with(tracing_subscriber::fmt::layer())
	// 	.try_init()?;

	let network_id = NetworkId::from("tdx-example");
	println!("Creating network with ID: {network_id}");
	let network = Network::builder(network_id)
		.with_discovery(discovery::Config::builder().no_auto_bootstrap())
		.build()
		.await
		.unwrap();
	println!("Network created. Waiting for it to be online...");
	network.online().await;
	println!("Network is online.");

	println!("Retrieving MR_TD measurement...");
	let quote = configfs_tsm::create_tdx_quote([0u8; 64]).unwrap();
	let quote = tdx_quote::Quote::from_bytes(quote.as_slice()).unwrap();

	let mrtd = network.tdx().mrtd().unwrap();
	println!("MR_TD measurement: {mrtd}");

	Ok(())
}
