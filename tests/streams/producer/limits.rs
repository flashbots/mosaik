use {
	super::*,
	crate::utils::*,
	core::{iter::once, task::Poll},
	futures::future::join_all,
	mosaik::*,
};

#[tokio::test]
async fn max_subs() -> anyhow::Result<()> {
	const MAX_CAPACITY: usize = 3;
	const CONSUMERS_COUNT: usize = 7;

	let network_id = NetworkId::random();

	// one producer node
	let n_p = Network::new(network_id).await?;

	// five consumer nodes
	let mut n_cs =
		join_all((0..CONSUMERS_COUNT).map(|_| Network::new(network_id)))
			.await
			.into_iter()
			.collect::<Result<Vec<_>, _>>()?;

	// build producer with max subscribers limit
	let prod = n_p
		.streams()
		.producer::<Data1>()
		.with_max_subscribers(MAX_CAPACITY)
		.build()?;

	assert!(prod.config().max_subscribers == MAX_CAPACITY);
	assert!(prod.config().max_subscribers < CONSUMERS_COUNT);

	let consumers = n_cs
		.iter()
		.map(|n| n.streams().consume::<Data1>())
		.collect::<Vec<_>>();

	// wait for all producers and consumers to be ready
	join_all(consumers.iter().map(|c| timeout_s(1, c.when().online()))).await;
	tracing::debug!("all consumers are online");

	// kick off full discovery
	discover_all(n_cs.iter().chain(once(&n_p))).await?;
	tracing::debug!("Full cross-network catalog sync complete");

	// wait for subscriptions to settle
	timeout_s(3, prod.when().subscribed().minimum_of(MAX_CAPACITY)).await?;
	tracing::debug!("Producer has reached max subscriptions");

	// should have only 3 subscribers due to limit
	assert_eq!(prod.consumers().count(), MAX_CAPACITY);

	// wait a bit to give it some time to process any late subscriptions
	sleep_s(2).await;

	// ensure that no new subscriptions were added
	assert_eq!(prod.consumers().count(), MAX_CAPACITY);

	// remember which consumers are subscribed and which are not
	let subscribed = prod.consumers().map(|s| *s.peer().id()).collect::<Vec<_>>();
	let unsubscribed = n_cs
		.iter()
		.map(|n| n.local().id())
		.filter(|id| !subscribed.contains(id))
		.collect::<Vec<_>>();

	assert_eq!(subscribed.len(), MAX_CAPACITY);
	assert_eq!(unsubscribed.len(), CONSUMERS_COUNT - MAX_CAPACITY);

	let condition = prod.when().subscribed().minimum_of(MAX_CAPACITY);
	let unmet = condition.clone().unmet();

	// ensure that the producer is still at max capacity
	assert_eq!(poll_once(&mut condition.clone()), Poll::Ready(()));
	assert_eq!(poll_once(&mut unmet.clone()), Poll::Pending);

	// terminate one of the subscribed consumers network.
	n_cs.retain(|n| n.local().id() != subscribed[0]);
	tracing::info!("Terminated consumer node with id {}", subscribed[0]);

	// wait for the producer to detect the dropped connection
	timeout_s(2, unmet).await.unwrap();
	tracing::debug!("Producer detected dropped consumer");

	// one of the unsubscribed consumers should be able to subscribe now
	timeout_s(2, condition).await.unwrap();
	tracing::debug!("Producer accepted new consumer after capacity freed");

	// ensure that the new set of connected consumers contain 2 of the old
	// subscribers and one new subscriber
	let new_subscribed =
		prod.consumers().map(|s| *s.peer().id()).collect::<Vec<_>>();
	let common = new_subscribed
		.iter()
		.filter(|id| subscribed.contains(id))
		.count();
	assert_eq!(common, MAX_CAPACITY - 1);
	let new_subscriber = new_subscribed
		.iter()
		.find(|id| !subscribed.contains(id))
		.expect("expected to find new subscriber");
	assert!(unsubscribed.contains(new_subscriber));

	Ok(())
}

/// This test verifies that the producer's `online_when` condition is
/// correctly applied to control when the producer is allowed to publish data.
///
/// Producers in this test do not place any restrictions on which consumers
/// can subscribe to them; all consumers are allowed to connect, but publishing
/// is controlled by the `online_when` condition.
#[tokio::test]
async fn online_when() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	// publishes data1 and data2 streams
	let n0 = Network::new(network_id).await?;

	// require at least two subscribers to allow publishing
	let p0_1 = n0
		.streams()
		.producer::<Data1>()
		.online_when(|c| c.minimum_of(2))
		.build()?;

	// default publish_if allows publishing with at least one subscriber
	let p0_2 = n0.streams().produce::<Data2>();

	// publishes data1 stream
	let n1 = Network::new(network_id).await?;

	// allow publishing on p1_1 only if there are at least two subscribers with
	// tag 'tag2'
	let p1_1 = n1
		.streams()
		.producer::<Data1>()
		.online_when(|c| c.with_tags("tag2").minimum_of(2))
		.build()?;

	// spin up a consumer node with no tags
	let n2 = Network::new(network_id).await?;
	n2.discovery().dial(n0.local().addr()).await;
	n2.discovery().dial(n1.local().addr()).await;

	let c2_1 = n2.streams().consume::<Data1>();
	let c2_2 = n2.streams().consume::<Data2>();

	// ensure that consumers subscribe to both producers
	timeout_s(2, c2_1.when().subscribed().minimum_of(2)).await?;
	timeout_s(2, c2_2.when().subscribed().minimum_of(1)).await?;
	tracing::debug!("Consumer n2 subscribed to all available producers");

	// should not be able to publish yet, only 1 subscriber
	timeout_s(2, p0_1.when().subscribed().minimum_of(1)).await?;
	timeout_s(1, p0_1.when().offline()).await?;
	assert!(!p0_1.is_online());

	// should be able to publish, has 1 subscriber
	timeout_s(2, p0_2.when().subscribed().minimum_of(1)).await?;
	timeout_s(2, p0_2.when().online()).await?;
	assert!(p0_2.is_online());

	// should not be able to publish yet, no subscribers with tag 'tag2'
	timeout_s(2, p1_1.when().subscribed().minimum_of(1)).await?;
	timeout_s(1, p1_1.when().offline()).await?;
	assert!(!p1_1.is_online());

	// spin up another consumer without the required tag
	let n3 = Network::new(network_id).await?;
	n3.discovery().dial(n0.local().addr()).await;
	n3.discovery().dial(n1.local().addr()).await;

	let c3_1 = n3.streams().consume::<Data1>();

	// ensure that the new consumer subscribes to both producers
	timeout_s(2, c3_1.when().subscribed().minimum_of(2)).await?;
	tracing::debug!("Consumer n3 subscribed to all available producers");

	// p0_1 should now be able to publish, has 2 subscribers
	timeout_s(2, p0_1.when().subscribed().minimum_of(2)).await?;
	timeout_s(2, p0_1.when().online()).await?;
	assert!(p0_1.is_online());

	// p1_1 should still not be able to publish, no subscribers with tag 'tag2'
	timeout_s(2, p1_1.when().subscribed().minimum_of(2)).await?;
	assert!(!p1_1.is_online());

	// spin up a tagged consumer
	let n4 = Network::builder(network_id)
		.with_discovery(
			discovery::Config::builder() //
				.with_tags("tag2"),
		)
		.build()
		.await?;
	n4.discovery().dial(n0.local().addr()).await;
	n4.discovery().dial(n1.local().addr()).await;

	let c4_1 = n4.streams().consume::<Data1>();

	// wait for tagged consumers to subscribe
	timeout_s(2, c4_1.when().subscribed().minimum_of(2)).await?;
	tracing::debug!("Consumer n4 subscribed to all available producers");

	// p1_1 should see the new subscriber but still not be able to publish,
	// only 1 of its subscribers has the required tag
	timeout_s(2, p1_1.when().subscribed().minimum_of(3)).await?;
	timeout_s(1, p1_1.when().offline()).await?;
	assert!(!p1_1.is_online());

	let n5 = Network::builder(network_id)
		.with_discovery(
			discovery::Config::builder() //
				.with_tags("tag2"),
		)
		.build()
		.await?;

	n5.discovery().dial(n0.local().addr()).await;
	n5.discovery().dial(n1.local().addr()).await;

	let c5_1 = n5.streams().consume::<Data1>();

	// wait for tagged consumers to subscribe
	timeout_s(2, c5_1.when().subscribed().minimum_of(2)).await?;
	tracing::debug!("Consumer n5 subscribed to all available producers");

	// p1_1 should now be able to publish, has 2 subscribers with required tag
	timeout_s(2, p1_1.when().subscribed().minimum_of(4)).await?;
	timeout_s(2, p1_1.when().online()).await?;
	assert!(p1_1.is_online());

	Ok(())
}
