use {
	backoff::{SystemClock, exponential::ExponentialBackoffBuilder},
	core::{task::Poll, time::Duration},
	futures::{SinkExt, StreamExt},
	mosaik::{test_utils::poll_once, *},
	serde::{Deserialize, Serialize},
	tokio::time::timeout,
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

	// 1 consumer, 0 producers
	let c1 = n1.streams().consume::<Data1>();

	let mut condition1 = c1.when().subscribed();
	let mut condition2 = c1.when().subscribed().to_at_least(2);
	let mut condition3 = condition2.clone().not(); // inverse of condition2

	assert!(!condition1.is_condition_met());
	assert!(!condition2.is_condition_met());
	assert_eq!(poll_once(&mut condition1), Poll::Pending);
	assert_eq!(poll_once(&mut condition2), Poll::Pending);

	// 1 consumer, 1 producer
	let _p0 = n0.streams().produce::<Data1>();

	// should resolve because we have 1 producer now
	timeout(Duration::from_secs(3), &mut condition1)
		.await
		.expect("timeout waiting for condition1");
	tracing::debug!("consumer subscribed with at least 1 producer");

	assert!(condition1.is_condition_met());
	assert!(!condition2.is_condition_met());
	assert!(condition3.is_condition_met());
	assert_eq!(poll_once(&mut condition2), Poll::Pending);

	// 1 consumer, 2 producers
	let _p2 = n2.streams().produce::<Data1>();

	// should resolve because we have 2 producers now
	timeout(Duration::from_secs(3), &mut condition2)
		.await
		.expect("timeout waiting for condition2");
	tracing::debug!("consumer subscribed with at least 2 producers");

	assert!(condition1.is_condition_met());
	assert!(condition2.is_condition_met());
	assert!(!condition3.is_condition_met());

	// clones resolve immediately if the condition is met
	let mut cond1_clone = condition1.clone();
	assert_eq!(poll_once(&mut cond1_clone), Poll::Ready(()));

	let mut cond2_clone = condition2.clone();
	assert_eq!(poll_once(&mut cond2_clone), Poll::Ready(()));

	// if we drop one producer, condition2 should go back to pending
	assert!(condition2.is_condition_met());
	assert!(!condition3.is_condition_met());

	assert_eq!(poll_once(&mut condition3.clone()), Poll::Pending);

	tracing::debug!("dropping n2");
	drop(n2);

	timeout(Duration::from_secs(3), &mut condition3)
		.await
		.expect("timeout waiting for inverse condition2");

	tracing::debug!("consumer no longer subscribed with at least 2 producers");

	assert!(!condition2.is_condition_met());
	assert!(condition3.is_condition_met());
	assert_eq!(poll_once(&mut condition2.clone()), Poll::Pending);

	assert!(condition3.is_condition_met());
	assert_eq!(poll_once(&mut condition3.clone()), Poll::Ready(()));

	let n3 = Network::new(network_id).await?;
	n3.discovery().dial(n0.local().id()).await;

	let _p3 = n3.streams().produce::<Data1>();

	timeout(Duration::from_secs(3), &mut condition2)
		.await
		.expect("timeout waiting for condition2 after n3 producer");
	tracing::debug!("consumer subscribed with at least 2 producers again");

	assert!(condition2.is_condition_met());
	assert!(!condition3.is_condition_met());
	assert_eq!(poll_once(&mut condition2.clone()), Poll::Ready(()));
	assert_eq!(poll_once(&mut condition3.clone()), Poll::Pending);

	Ok(())
}
