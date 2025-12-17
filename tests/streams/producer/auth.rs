use {
	super::*,
	crate::utils::{discover_all, timeout_s},
	mosaik::*,
};

/// This test verifies that producers can authenticate consumers based on tags
/// and refuse subscriptions from consumers that do not meet the criteria.
#[tokio::test]
async fn by_tag() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	// producers
	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;

	// restrict to consumers with tag 'tag2'
	let p0 = n0
		.streams()
		.producer::<Data1>()
		.accept_if(|peer| peer.tags().contains(&"tag2".into()))
		.build()?;

	// unrestricted producer, accepts all consumers
	let p1 = n1.streams().produce::<Data1>();

	// consumers
	let n3 = Network::builder(network_id)
		.with_discovery(
			discovery::Config::builder() //
				.with_tags(["tag1", "tag2"]),
		)
		.build()
		.await?;

	let n4 = Network::builder(network_id)
		.with_discovery(
			discovery::Config::builder() //
				.with_tags(["tag3", "tag4"]),
		)
		.build()
		.await?;

	let c3 = n3.streams().consume::<Data1>();
	let c4 = n4.streams().consume::<Data1>();

	// sync discovery catalogs
	discover_all([&n0, &n1, &n3, &n4]).await?;

	// c3 should be subscribed to p0 and p1 (n0 and n1), as it has the required
	// tag to connect to p0. c4 should be subscribed only to p1 because it lacks
	// the required tag to connect to p0.
	timeout_s(2, c3.when().subscribed().minimum_of(2)).await?;
	timeout_s(2, c4.when().subscribed().minimum_of(1)).await?;

	// verify producer subscriptions
	timeout_s(2, p0.when().subscribed().minimum_of(1)).await?;
	timeout_s(2, p1.when().subscribed().minimum_of(2)).await?;

	let p0_subs = p0.consumers().map(|s| *s.peer().id()).collect::<Vec<_>>();
	let p1_subs = p1.consumers().map(|s| *s.peer().id()).collect::<Vec<_>>();

	assert_eq!(p0_subs.len(), 1);
	assert!(p0_subs.contains(&n3.local().id()));

	assert_eq!(p1_subs.len(), 2);
	assert!(p1_subs.contains(&n3.local().id()));
	assert!(p1_subs.contains(&n4.local().id()));

	Ok(())
}
