use mosaik::{
	primitives::{Pretty, Short},
	tdx::TdxTicket,
	*,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	let network_id = NetworkId::from("tdx-example2");
	let network = Network::new(network_id).await?;

	println!("Network {network_id} created...");

	println!("Generating TDX ticket...");
	let ticket = network.tdx().ticket()?;
	println!("Generic ticket generated: {:?}", Short(&ticket));

	let tdx_ticket: TdxTicket = ticket.try_into()?;
	println!("TDX ticket: {:#?}", Pretty(&tdx_ticket));

	println!(
		"TDX ticket quote signatures verified: {}",
		tdx_ticket.quote().verify().is_ok()
	);

	let mrtd = network.tdx().mrtd().unwrap();
	println!("MR_TD measurement: {mrtd}");

	let measurements = network.tdx().measurements()?;
	println!("All measurements: {measurements:#?}");

	let rtmr0 = network.tdx().rtmr0().unwrap();
	println!("RTMR0 measurement: {rtmr0}");

	let rtmr1 = tdx_ticket.measurements().rtmr1();
	println!("RTMR1 measurement: {rtmr1}");

	let rtmr2 = tdx_ticket.measurements().rtmr2();
	println!("RTMR2 measurement: {rtmr2}");

	println!("ticket peer id: {}", tdx_ticket.peer_id());
	println!("ticket network id: {}", tdx_ticket.network_id());
	println!("ticket started at: {}", tdx_ticket.started_at());
	println!("ticket expiration: {:?}", tdx_ticket.expiration());

	println!("All done!");
	Ok(())
}
