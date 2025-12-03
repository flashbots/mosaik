use {
	backoff::{SystemClock, exponential::ExponentialBackoffBuilder},
	core::time::Duration,
	futures::{SinkExt, StreamExt},
	mosaik::*,
	serde::{Deserialize, Serialize},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Data1(pub String);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Data2(pub String);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Data3(pub String);

#[tokio::test]
async fn api_design_basic() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let n0 = Network::new(network_id).await.unwrap();

	let n1 = Network::builder(network_id)
		.with_discovery(
			discovery::Config::builder() //
				.with_bootstrap(n0.local().id()),
		)
		.with_streams(
			streams::Config::builder().with_backoff(
				ExponentialBackoffBuilder::<SystemClock>::default()
					.with_max_elapsed_time(Some(Duration::from_secs(10)))
					.build(),
			),
		)
		.build()
		.await?;

	let n2 = Network::builder(network_id)
		.with_discovery(
			discovery::Config::builder() //
				.with_bootstrap(n0.local().id()),
		)
		.with_streams(
			streams::Config::builder().with_backoff(
				ExponentialBackoffBuilder::<SystemClock>::default()
					.with_max_elapsed_time(Some(Duration::from_secs(10)))
					.build(),
			),
		)
		.build()
		.await?;

	// node0
	let mut p0 = n0.streams().produce::<Data1>();
	let p1 = n0.streams().produce::<Data2>();

	// node1
	let c1 = n1.streams().consume::<Data1>();
	let p1 = n1.streams().produce::<Data3>();

	// node2
	let mut c2a = n2.streams().consume::<Data1>();
	let c2b = n2.streams().consume::<Data2>();
	let c2c = n2.streams().consume::<Data3>();

	p0.status().subscribed().by_at_least(2).await;
	p0.send(Data1("One".into())).await.unwrap();

	let recv_c2a = c2a.next().await;
	assert_eq!(recv_c2a, Some(Data1("One".into())));

	Ok(())
}
