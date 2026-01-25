use {super::*, crate::utils::discover_all, futures::SinkExt, mosaik::*};

#[tokio::test]
async fn smoke() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	// start with one producer and one consumer
	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;

	let mut p0 = n0
		.streams()
		.producer::<NoncesUpdate>()
		//.with_store(NoncesStore::default())
		.build()?;

	let mut c1 = n1
		.streams()
		.consumer::<NoncesUpdate>()
		//.with_store(NoncesStore::default())
		.build();

	discover_all([&n0, &n1]).await?;

	// produce and consume some updates
	let update1 = NoncesUpdate::new(1, [
		(Address::from([11u8; 20]), 5),
		(Address::from([22u8; 20]), 6),
		(Address::from([88u8; 20]), 8),
	]);

	let update2 = NoncesUpdate::new(2, [
		(Address::from([110u8; 20]), 6),
		(Address::from([22u8; 20]), 7),
		(Address::from([21u8; 20]), 1),
		(Address::from([33u8; 20]), 1),
	]);

	p0.send(update1).await?;
	p0.send(update2).await?;

	let received1 = c1.recv().await.unwrap();
	let received2 = c1.recv().await.unwrap();

	tracing::debug!("received1: {received1:#?}");
	tracing::debug!("received2: {received2:#?}");

	// add a second consumer after some data has already been produced
	// and verify it can catch up via replay
	let n2 = Network::new(network_id).await?;
	let _c2 = n2.streams().consume::<NoncesUpdate>();

	discover_all([&n0, &n1, &n2]).await?;

	Ok(())
}
