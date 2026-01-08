use {
	super::*,
	crate::utils::timeout_s,
	futures::{SinkExt, StreamExt, join},
	mosaik::*,
	tokio::sync::watch,
};

#[tokio::test]
async fn send_recv() -> anyhow::Result<()> {
	const MSG_COUNT: usize = 100;

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

	let mut c1 = n1.streams().consume::<Data1>();
	let mut c2 = n2.streams().consume::<Data1>();

	c1.when().subscribed().await;
	c2.when().subscribed().await;
	tracing::debug!("consumers successfully subscribed to producers");

	join!(
		async {
			for _ in 0..MSG_COUNT {
				let msg = Data1("hello from n0".into());
				p0.send(msg).await.unwrap();
			}
		},
		async {
			let (sum, mut sum_watch) = watch::channel(0);

			loop {
				tokio::select! {
					recv = c1.next() => {
						let msg = recv.expect("expected message from c1");
						assert_eq!(msg, Data1("hello from n0".into()));
						sum.send_modify(|s| *s += 1);
					},
					recv = c2.next() => {
						let msg = recv.expect("expected message from c2");
						assert_eq!(msg, Data1("hello from n0".into()));
						sum.send_modify(|s| *s += 1);
					}
					_ = sum_watch.wait_for(|s| *s >= MSG_COUNT * 2) => {
						tracing::debug!("received {} messages, ending test", MSG_COUNT * 2);
						break;
					}
				}
			}
		}
	);

	assert_eq!(c1.stats().datums(), MSG_COUNT);
	assert_eq!(c2.stats().datums(), MSG_COUNT);

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
async fn custom_stream_id() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;

	// with custom stream ids we can have multiple producers of the
	// same datum type on the same node differentiated by stream id
	let mut p0_1 = n0
		.streams()
		.producer::<Data1>()
		.with_stream_id("stream1234")
		.build()?;

	let mut p0_2 = n0
		.streams()
		.producer::<Data1>()
		.with_stream_id("stream5678")
		.build()?;

	let mut c1_1 = n1
		.streams()
		.consumer::<Data1>()
		.with_stream_id("stream1234")
		.build();

	let mut c1_2 = n1
		.streams()
		.consumer::<Data1>()
		.with_stream_id("stream5678")
		.build();

	n1.discovery().dial(n0.local().addr()).await;

	timeout_s(3, c1_1.when().subscribed()).await?;
	timeout_s(3, c1_2.when().subscribed()).await?;

	tracing::debug!("consumers successfully subscribed to producers");

	timeout_s(3, p0_1.send(Data1("hello1234".into()))).await??;
	timeout_s(3, p0_2.send(Data1("hello5678".into()))).await??;

	let msg0 = timeout_s(3, c1_1.next())
		.await?
		.expect("expected message from c1_1");
	assert_eq!(msg0, Data1("hello1234".into()));

	let msg1 = timeout_s(3, c1_2.next())
		.await?
		.expect("expected message from c1_2");
	assert_eq!(msg1, Data1("hello5678".into()));

	Ok(())
}
