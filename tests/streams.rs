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

	let n3 = Network::new(network_id).await?;
	let p3 = n3.streams().produce::<Data3>();
	n2.discovery().insert(n3.discovery().me());

	// node0
	let mut p0 = n0.streams().produce::<Data1>();
	let p1 = n0.streams().produce::<Data2>();

	// node1
	let c1 = n1.streams().consume::<Data1>();
	// let p1 = n1.streams().produce::<Data3>();

	// node2
	let mut c2a = n2.streams().consume::<Data1>();
	let c2b = n2.streams().consume::<Data2>();
	let c2c = n2.streams().consume::<Data3>();

	p0.when().subscribed().by_at_least(2).await;
	p0.send(Data1("One".into())).await.unwrap();

	let recv_c2a = c2a.next().await;
	assert_eq!(recv_c2a, Some(Data1("One".into())));

	Ok(())
}

/// This test ensures that a producer receiving a subscription request from
/// an unknown consumer peer properly rejects the request with
/// [`CloseReason::UnknownPeer`], prompting the consumer to send its catalog
/// sync request that contains its own peer entry.
#[tokio::test]
async fn subscription_triggers_catalog_sync() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	// n0 does not know n1 initially
	let n0 = Network::builder(network_id)
		.with_discovery(
			discovery::Config::builder()
				.with_tags("tag1")
				.with_tags(["tag2", "tag3"]),
		)
		.build()
		.await?;
	let _p0 = n0.streams().produce::<Data1>();

	// n1 knows n0 through local catalog entry
	let n1 = Network::new(network_id).await?;
	n1.discovery().insert(n0.discovery().me());

	// n1 should have n0 in its catalog as an unsigned peer
	assert_eq!(n1.discovery().catalog().unsigned_peers().count(), 1);

	// n1 consumes Data1, triggering a subscription request to n0
	// which initially gets rejected with CloseReason::UnknownPeer
	// then a full catalog sync is performed, and finally the subscription
	// request is retried and accepted. N1 should have n0 as a signed peer
	// after the sync.
	let c1 = n1.streams().consume::<Data1>();

	c1.when().subscribed().await;
	tracing::debug!("consumer successfully subscribed to producer");

	// n1 should have n0 in its catalog as a signed peer
	assert_eq!(n1.discovery().catalog().signed_peers().count(), 1);

	// n1 should see n0's tags in its catalog
	let n1_catalog = n1.discovery().catalog();
	let n0_info = n1_catalog
		.get(&n0.local().id())
		.expect("n0 should be in n1's catalog");

	assert!(n0_info.tags().contains(&"tag1".into()));
	assert!(n0_info.tags().contains(&"tag2".into()));
	assert!(n0_info.tags().contains(&"tag3".into()));

	Ok(())
}

#[tokio::test]
async fn consumer_subscription_conditions() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	let n2 = Network::new(network_id).await?;

	tracing::debug!("n0: {}", n0.local().id());
	tracing::debug!("n1: {}", n1.local().id());
	tracing::debug!("n2: {}", n2.local().id());

	// discover peers
	n1.discovery().dial(n0.local().id()).await;
	n2.discovery().dial(n0.local().id()).await;

	// 1 producer and 1 consumer
	let _p0 = n0.streams().produce::<Data1>();
	let c1 = n1.streams().consume::<Data1>();

	// consumer waits until at least 1 producer is available
	let condition1 = c1.when().subscribed();

	condition1.clone().await;
	tracing::debug!("consumer subscribed with at least 1 producer");
	assert!(condition1.is_condition_met());

	// 2 producers and 1 consumer
	let _p2 = n2.streams().produce::<Data1>();
	tokio::time::sleep(Duration::from_millis(2000)).await;
	let condition2 = c1.when().subscribed().to_at_least(2);

	condition2.clone().await;
	tracing::debug!("consumer subscribed with at least 2 producers");
	assert!(condition2.is_condition_met());
	assert!(condition1.is_condition_met());
	Ok(())
}
