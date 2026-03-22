use {
	super::*,
	crate::utils::{discover_all, poll_once, timeout_s},
	core::task::Poll,
	mosaik::*,
};

/// This tests the `When` conditions for stream consumers that allows public API
/// users to await certain subscription states of the consumer. The analogous
/// version of this test for producers is in `streams::producer::when::smoke`.
#[tokio::test]
async fn smoke() -> anyhow::Result<()> {
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
	let mut condition2 = c1.when().subscribed().minimum_of(2);
	let mut condition3 = condition2.clone().unmet(); // inverse of condition2
	let condition4 = c1.when().unsubscribed(); // inverse of condition1

	assert!(!condition1.is_condition_met());
	assert!(!condition2.is_condition_met());
	assert!(condition3.is_condition_met());
	assert!(condition4.is_condition_met());

	assert_eq!(poll_once(&mut condition1), Poll::Pending);
	assert_eq!(poll_once(&mut condition2), Poll::Pending);
	assert_eq!(poll_once(&mut condition3.clone()), Poll::Ready(()));
	assert_eq!(poll_once(&mut condition4.clone()), Poll::Ready(()));

	// 1 consumer, 1 producer
	let _p0 = n0.streams().produce::<Data1>();

	// should resolve because we have 1 producer now
	timeout_s(3, &mut condition1)
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
	timeout_s(3, &mut condition2)
		.await
		.expect("timeout waiting for condition2");
	tracing::debug!("consumer subscribed with at least 2 producers");

	assert!(condition1.is_condition_met());
	assert!(condition2.is_condition_met());
	assert!(!condition3.is_condition_met());
	assert!(!condition4.is_condition_met());

	// clones resolve immediately if the condition is met
	let mut cond1_clone = condition1.clone();
	assert_eq!(poll_once(&mut cond1_clone), Poll::Ready(()));

	let mut cond2_clone = condition2.clone();
	assert_eq!(poll_once(&mut cond2_clone), Poll::Ready(()));

	// if we drop one producer, condition2 should go back to pending
	// and its inverse (condition3) should resolve
	assert!(condition1.is_condition_met());
	assert!(condition2.is_condition_met());
	assert!(!condition3.is_condition_met());
	assert!(!condition4.is_condition_met());
	assert_eq!(poll_once(&mut condition3.clone()), Poll::Pending);

	tracing::debug!("dropping n2");
	drop(n2);

	// should resolve because we are back to 1 producer
	timeout_s(3, &mut condition3)
		.await
		.expect("timeout waiting for inverse condition2");

	tracing::debug!("consumer no longer subscribed with at least 2 producers");

	assert!(!condition2.is_condition_met());
	assert!(condition3.is_condition_met());
	assert_eq!(poll_once(&mut condition2.clone()), Poll::Pending);

	assert!(condition3.is_condition_met());
	assert_eq!(poll_once(&mut condition3.clone()), Poll::Ready(()));

	// if we add another producer, condition2 should resolve again because its
	// criteria is met, and condition3 should go back to pending
	let n3 = Network::new(network_id).await?;
	tracing::debug!("n3: {}", n3.local().id());
	n3.discovery().dial(n0.local().id()).await;

	// add producer on n3 for Data1 stream
	let _p3 = n3.streams().produce::<Data1>();

	timeout_s(3, &mut condition2)
		.await
		.expect("timeout waiting for condition2 after n3 producer");
	tracing::debug!("consumer subscribed with at least 2 producers again");

	assert!(condition2.is_condition_met());
	assert!(!condition3.is_condition_met());
	assert_eq!(poll_once(&mut condition2.clone()), Poll::Ready(()));
	assert_eq!(poll_once(&mut condition3.clone()), Poll::Pending);

	Ok(())
}

/// Verifies that a consumer configured with `online_when` is not considered
/// online until the specified conditions are met, and transitions back to
/// offline when they are no longer satisfied.
#[tokio::test]
async fn online_when() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::builder(network_id)
		.with_discovery(
			discovery::Config::builder() //
				.with_tags(["t1", "t2"]),
		)
		.build()
		.await?;
	let n2 = Network::builder(network_id)
		.with_discovery(
			discovery::Config::builder() //
				.with_tags(["t1", "t2"]),
		)
		.build()
		.await?;

	tracing::debug!("n0: {}", n0.local().id());
	tracing::debug!("n1: {}", n1.local().id());
	tracing::debug!("n2: {}", n2.local().id());

	// Consumer requires at least 2 producers with tags "t1" and "t2"
	let c0 = n0
		.streams()
		.consumer::<Data1>()
		.online_when(|c| c.minimum_of(2).with_tags(["t1", "t2"]))
		.build();

	// Consumer should not be online yet (no producers)
	assert!(!c0.is_online());

	// Discover peers
	timeout_s(10, discover_all([&n0, &n1, &n2])).await??;

	// Start one producer — still not enough for the condition
	let _p1 = n1.streams().produce::<Data1>();

	timeout_s(3, c0.when().subscribed()).await?;
	assert!(!c0.is_online(), "should not be online with only 1 producer");

	// Start second producer — condition should now be met
	let _p2 = n2.streams().produce::<Data1>();

	timeout_s(3, c0.when().online()).await?;
	assert!(c0.is_online(), "should be online with 2 tagged producers");

	// Drop one producer — condition should no longer be met
	drop(n2);

	timeout_s(3, c0.when().offline()).await?;
	assert!(!c0.is_online(), "should be offline after losing a producer");

	Ok(())
}

/// Verifies that a consumer transitions to offline when a producer is removed
/// from the discovery catalog (e.g., tag removal) rather than via connection
/// termination. This exercises the `on_catalog_update` path where producers
/// are disconnected because they no longer satisfy eligibility criteria.
#[tokio::test]
async fn online_when_catalog_removal() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::builder(network_id)
		.with_discovery(
			discovery::Config::builder() //
				.with_tags(["trusted"]),
		)
		.build()
		.await?;
	let n2 = Network::builder(network_id)
		.with_discovery(
			discovery::Config::builder() //
				.with_tags(["trusted"]),
		)
		.build()
		.await?;

	// Consumer requires 2 producers tagged "trusted" and only subscribes
	// to those with the "trusted" tag.
	let c0 = n0
		.streams()
		.consumer::<Data1>()
		.subscribe_if(|peer| peer.tags().contains(&"trusted".into()))
		.online_when(|c| c.minimum_of(2))
		.build();

	let _p1 = n1.streams().produce::<Data1>();
	let _p2 = n2.streams().produce::<Data1>();

	timeout_s(5, discover_all([&n0, &n1, &n2])).await??;

	// Both producers are connected — consumer should be online
	timeout_s(3, c0.when().online()).await?;
	assert!(c0.is_online(), "should be online with 2 tagged producers");

	// Remove the "trusted" tag from n1 and feed the update to n0.
	// This triggers catalog-based disconnection (not connection termination).
	n1.discovery().remove_tags("trusted");
	n0.discovery().feed(n1.discovery().me());

	// Consumer should transition to offline because only 1 producer remains
	timeout_s(3, c0.when().offline()).await?;
	assert!(
		!c0.is_online(),
		"should be offline after catalog-based producer removal"
	);

	Ok(())
}

/// Verifies that a consumer with the default `online_when` (no conditions)
/// is immediately online even without any producers.
#[tokio::test]
async fn online_when_default_is_immediate() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let n0 = Network::new(network_id).await?;

	let c0 = n0.streams().consume::<Data1>();

	// Default consumer should be online immediately
	assert!(c0.is_online());

	// The online future should resolve immediately
	timeout_s(1, c0.when().online()).await?;

	Ok(())
}
