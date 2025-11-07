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

	let n3 = Network::new(network_id.clone()).await?;
	let mut c3a = n3.consume::<Data1>();
	let c3b = n3.consume::<Data2>();
	let c3c = n3.consume::<Data3>();

	p0.send(Data1("One".into())).await?;

	let recv_c3a = c3a.next().await;

	assert_eq!(recv_c3a, Some(Data1("One".into())));

	Ok(())
}
