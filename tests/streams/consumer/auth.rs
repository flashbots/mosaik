use {
	super::*,
	crate::utils::{discover_all, timeout_s},
	mosaik::{primitives::Short, *},
};

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

	// This consumer will only attempt to subscribe to
	// producers that have 'tag2' in their tags list.
	// This represents any arbitrary authentication logic based on
	// peer attributes.
	let c0_1 = n0
		.streams()
		.consumer::<Data1>()
		.subscribe_if(|peer| peer.tags().contains(&"tag2".into()))
		.build();

	// This consumer will attempt to subscribe to
	// all discovered producers.
	let c0_2 = n0.streams().consumer::<Data1>().build();

	let p1 = n1.streams().produce::<Data1>();
	let p2 = n2.streams().produce::<Data1>();

	// wait for producers and consumer to be ready
	timeout_s(1, c0_1.when().ready()).await?;
	timeout_s(1, c0_2.when().ready()).await?;
	timeout_s(1, p1.when().ready()).await?;
	timeout_s(1, p2.when().ready()).await?;

	// sync discovery catalogs
	discover_all(&[&n0, &n1, &n2]).await?;

	// c0_1 should only be subscribed to p1 (n1), as it's the only producer
	// that has the required tag. c0_2 should be subscribed to both p1 and p2
	// because it has no restrictions.
	timeout_s(2, c0_1.when().subscribed()).await?;
	timeout_s(2, c0_2.when().subscribed().to_at_least(2)).await?;

	// verify that c0_1 is only subscribed to n1
	let subs = c0_1.producers().collect::<Vec<_>>();
	assert_eq!(subs.len(), 1);
	assert_eq!(*subs[0].peer().id(), n1.local().id());

	tracing::debug!("c0_1 is subscribed to the following producers:");
	for producer in subs {
		tracing::debug!(
			" - {}, stats: {}",
			Short(producer.peer()),
			producer.stats()
		);
	}

	// verify that c0_2 is subscribed to both n1 and n2
	let subs = c0_2.producers().collect::<Vec<_>>();
	assert_eq!(subs.len(), 2);
	assert!(subs.iter().any(|p| *p.peer().id() == n1.local().id()));
	assert!(subs.iter().any(|p| *p.peer().id() == n2.local().id()));

	tracing::debug!("c0_2 is subscribed to the following producers:");
	for producer in subs {
		tracing::debug!(
			" - {}, stats: {}",
			Short(producer.peer()),
			producer.stats()
		);
	}

	Ok(())
}
