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
async fn api_design() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let n0 = Network::new(network_id.clone()).await?;
	let mut p0 = n0.produce::<Data1>();
	let p1 = n0.produce::<Data2>();

	let n1 = Network::new(network_id.clone()).await?;
	let c1 = n1.consume::<Data1>();
	let p1 = n1.produce::<Data3>();

	let n2 = Network::new(network_id.clone()).await?;
	let mut c2a = n2.consume::<Data1>();
	let c2b = n2.consume::<Data2>();
	let c2c = n2.consume::<Data3>();

	n1.discovery().dial(n0.local().addr()).await?;
	n2.discovery().dial(n0.local().addr()).await?;

	p0.send(Data1("One".into())).await?;

	let recv_c2a = c2a.next().await;

	assert_eq!(recv_c2a, Some(Data1("One".into())));
	Ok(())
}

#[tokio::test]
async fn api_design_manual_disc() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let n0 = Network::new(network_id.clone()).await?;
	let mut p0_1 = n0.produce::<Data1>();

	let n1 = Network::new(network_id.clone()).await?;
	let mut c1_1 = n1.consume::<Data1>();
	let mut p1_1 = n1.produce::<Data1>();

	let n2 = Network::new(network_id.clone()).await?;
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

	let n3 = Network::new(network_id.clone()).await?;
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
