use {
	crate::{
		Data1,
		utils::{discover_all, sleep_s, timeout_s},
	},
	futures::{SinkExt, StreamExt},
	mosaik::{
		tee::tdx::{NetworkTicketExt, TdxValidator},
		*,
	},
};

#[tokio::test]
async fn stream_consumer() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let (n0, n1, n2, n3) = tokio::try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
	)?;

	let n1_ticket = n1.tdx().ticket()?;
	let n2_ticket = n2.tdx().ticket()?;

	let validator = TdxValidator::new().require_mrtd(n1.tdx().mrtd()?);

	n1.discovery().add_ticket(n1_ticket);
	n2.discovery().add_ticket(n2_ticket);

	let mut p1 = n1.streams().produce::<Data1>();
	let _p2 = n2.streams().produce::<Data1>();
	let _p3 = n3.streams().produce::<Data1>();

	// Consumer with ticket validator
	let mut c0 = n0
		.streams()
		.consumer::<Data1>()
		.with_ticket_validator(validator)
		.build();

	timeout_s(5, discover_all([&n0, &n1, &n2, &n3])).await??;

	// Only n1 should be subscribed
	sleep_s(5).await;

	let producers = c0.producers().map(|p| *p.peer().id()).collect::<Vec<_>>();
	assert_eq!(producers.len(), 1);
	assert!(producers.contains(&n1.local().id()));

	// Data flows through the valid connection
	p1.send(Data1("hello".into())).await?;
	assert_eq!(timeout_s(2, c0.next()).await?, Some(Data1("hello".into())));

	Ok(())
}
