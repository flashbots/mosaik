use {
	mosaik::{tee::tdx::NetworkTicketExt, *},
	tdx_quote::{
		CertificationData,
		CertificationDataInner,
		Quote,
		QuoteVerificationError,
	},
};

/// Default URL for the host's PCCS, reachable from inside a QEMU
/// user-mode-networking guest at the gateway address.
const DEFAULT_PCCS_URL: &str = "https://10.0.2.2:8081";

/// Intel's public Provisioning Certification Service.
const INTEL_PCS_URL: &str = "https://api.trustedservices.intel.com";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	// use tracing_subscriber::prelude::*;
	// tracing_subscriber::registry()
	// 	.with(tracing_subscriber::fmt::layer())
	// 	.try_init()?;

	let network_id = NetworkId::from("tdx-example");
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

	Ok(())
}

fn hex_encode(bytes: &[u8]) -> String {
	bytes.iter().map(|b| format!("{b:02x}")).collect()
}
