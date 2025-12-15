use {
	super::*,
	futures::{SinkExt, StreamExt},
	mosaik::*,
};

#[tokio::test]
async fn send_recv_smoke() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	let n2 = Network::new(network_id).await?;

	tracing::debug!("n0: {}", n0.local().id());
	tracing::debug!("n1: {}", n1.local().id());
	tracing::debug!("n2: {}", n2.local().id());

	n1.discovery().dial(n0.local().id()).await;
	n2.discovery().dial(n0.local().id()).await;

	let mut p0 = n0.streams().produce::<Data1>();
	let mut p1 = n1.streams().produce::<Data2>();

	let mut c1 = n2.streams().consume::<Data1>();
	let mut c2 = n0.streams().consume::<Data2>();

	c1.when().subscribed().await;
	c2.when().subscribed().await;

	tracing::debug!("consumers successfully subscribed to producers");
	p0.send(Data1("hello from n0".into())).await?;
	p1.send(Data2("hello from n1".into())).await?;

	let msg0 = c1.next().await.expect("expected message from c1");
	let msg1 = c2.next().await.expect("expected message from c2");
	assert_eq!(msg0, Data1("hello from n0".into()));
	assert_eq!(msg1, Data2("hello from n1".into()));

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
