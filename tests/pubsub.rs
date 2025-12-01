use {
	futures::{SinkExt, StreamExt},
	mosaik::prelude::*,
	serde::{Deserialize, Serialize},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Data1(pub String);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Data2(pub String);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Data3(pub String);

#[tokio::test]
async fn api_design_auto_disc() {
	let network_id = NetworkId::random();

	let n0 = NetworkBuilder::new(network_id.clone())
		.build_and_run()
		.await
		.unwrap();
	let mut p0 = n0.produce::<Data1>();
	let _p1 = n0.produce::<Data2>();

	// let n1 = Network::run(network_id.clone()).await.unwrap();
	let n1 = NetworkBuilder::new(network_id.clone())
		.with_bootstrap_peer(n0.local().addr())
		.build_and_run()
		.await
		.unwrap();
	let _c1 = n1.consume::<Data1>();
	let _p1 = n1.produce::<Data3>();

	let n2 = NetworkBuilder::new(network_id.clone())
		.with_bootstrap_peer(n0.local().addr())
		.build_and_run()
		.await
		.unwrap();
	let mut c2a = n2.consume::<Data1>();
	let _c2b = n2.consume::<Data2>();
	let _c2c = n2.consume::<Data3>();

	n1.discovery().dial(n0.local().addr()).await.unwrap();
	n2.discovery().dial(n0.local().addr()).await.unwrap();

	p0.status().subscribed_at_least(2).await;
	p0.send(Data1("One".into())).await.unwrap();

	let recv_c2a = c2a.next().await;
	assert_eq!(recv_c2a, Some(Data1("One".into())));
}

#[tokio::test]
async fn api_design_manual_disc() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let n0 = NetworkBuilder::new(network_id.clone())
		.build_and_run()
		.await?;
	let mut p0_1 = n0.produce::<Data1>();

	let n1 = NetworkBuilder::new(network_id.clone())
		.build_and_run()
		.await?;
	let mut c1_1 = n1.consume::<Data1>();
	let mut p1_1 = n1.produce::<Data1>();

	let n2 = NetworkBuilder::new(network_id.clone())
		.build_and_run()
		.await?;
	let mut c2_1 = n2.consume::<Data1>();

	full_manual_disco(&[&n0, &n1, &n2]);

	p0_1.status().subscribed_at_least(2).await;
	p1_1.status().subscribed().await;
	c1_1.status().subscribed().await;
	c2_1.status().subscribed().await;

	p0_1.send(Data1("One".into())).await?;

	let recv_c2a = c2_1.next().await;
	let recv_c1 = c1_1.next().await;

	assert_eq!(recv_c2a, Some(Data1("One".into())));
	assert_eq!(recv_c1, Some(Data1("One".into())));

	p1_1.send(Data1("Three".into())).await?;
	let recv_c2 = c2_1.next().await;

	assert_eq!(recv_c2, Some(Data1("Three".into())));

	let n3 = NetworkBuilder::new(network_id.clone())
		.build_and_run()
		.await?;
	let mut c3_1 = n3.produce::<Data1>();

	full_manual_disco(&[&n1, &n2, &n3]);

	c3_1.status().subscribed_at_least(2).await;

	c3_1.send(Data1("Five".into())).await?;

	let recv_c1 = c1_1.next().await;
	assert_eq!(recv_c1, Some(Data1("Five".into())));

	let recv_c2 = c2_1.next().await;
	assert_eq!(recv_c2, Some(Data1("Five".into())));

	Ok(())
}

fn full_manual_disco(peers: &[&Network]) {
	for a in peers {
		for b in peers {
			if a.local().id() != b.local().id() {
				a.discovery().catalog().insert(b.local().info());
			}
		}
	}
}
