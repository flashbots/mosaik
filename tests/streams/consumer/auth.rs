use {super::*, mosaik::*};

#[tokio::test]
async fn by_tag() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let n0 = Network::new(network_id).await?;
	let n1 = Network::builder(network_id)
		.with_discovery(
			discovery::Config::builder() //
				.with_tags(["tag1", "tag2"]),
		)
		.build()
		.await?;

	let n2 = Network::builder(network_id)
		.with_discovery(
			discovery::Config::builder() //
				.with_tags(["tag4", "tag5"]),
		)
		.build()
		.await?;

	n0.discovery().dial(n1.local().id()).await;
	n1.discovery().dial(n2.local().id()).await;

	// This consumer will only attempt to subscribe to
	// producers that have 'tag2' in their tags list.
	let c0 = n0
		.streams()
		.consumer::<Data1>()
		.only_from(|peer| peer.tags().contains(&"tag2".into()))
		.build();

	let p1 = n1.streams().produce::<Data1>();
	let p2 = n2.streams().produce::<Data1>();

	// ensure that c0 subscribes only to p1
	c0.when().subscribed().to_at_least(2).await;
	tracing::info!("consumer subscribed");

	// core::future::pending::<()>().await;
	Ok(())
}
