use {
	mosaik::{tee::tdx::NetworkTicketExt, *},
	tdx_quote::Quote,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	let network_id = NetworkId::from("tdx-example2");
	let network = Network::new(network_id).await?;

	println!("Network {network_id} created.");

	println!("Retrieving TDX quote...");
	let raw_quote = configfs_tsm::create_tdx_quote([0u8; 64]).unwrap();

	let quote = Quote::from_bytes(raw_quote.as_slice()).unwrap();
	println!(
		"verifying quote: {}",
		hex_encode(&quote.verify().unwrap().to_sec1_bytes())
	);

	let mrtd = network.tdx().mrtd().unwrap();
	println!("MR_TD measurement: {mrtd}");

	let rtmr0 = network.tdx().rtmr0().unwrap();
	println!("RTMR0 measurement: {rtmr0}");

	let rtmr1 = network.tdx().rtmr1().unwrap();
	println!("RTMR1 measurement: {rtmr1}");

	let rtmr2 = network.tdx().rtmr2().unwrap();
	println!("RTMR2 measurement: {rtmr2}");

	Ok(())
}

fn hex_encode(bytes: &[u8]) -> String {
	bytes.iter().map(|b| format!("{b:02x}")).collect()
}
