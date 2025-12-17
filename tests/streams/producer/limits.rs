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
	timeout_s(1, prod.when().online()).await?;
	join_all(consumers.iter().map(|c| timeout_s(1, c.when().online()))).await;
	tracing::debug!("all producer and consumers are online");

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
