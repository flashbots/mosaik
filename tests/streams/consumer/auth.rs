use {
	super::*,
	crate::utils::{discover_all, timeout_s},
	mosaik::{discovery::PeerEntry, primitives::Short, *},
};

/// This test verifies that consumers can authenticate producers based on tags
/// and refuse subscriptions to producers that do not meet the criteria.
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

	// sync discovery catalogs
	discover_all([&n0, &n1, &n2]).await?;

	// c0_1 should only be subscribed to p1 (n1), as it's the only producer
	// that has the required tag. c0_2 should be subscribed to both p1 and p2
	// because it has no restrictions.
	timeout_s(2, c0_1.when().subscribed()).await?;
	timeout_s(2, c0_2.when().subscribed().minimum_of(2)).await?;
	timeout_s(2, p1.when().subscribed().minimum_of(2)).await?;
	timeout_s(2, p2.when().subscribed()).await?;

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

/// Verifies that consumers disconnect from producers whose tags change such
/// that the `subscribe_if` predicate no longer matches, and reconnect to
/// producers that gain matching tags.
#[tokio::test]
async fn dynamic_tag_reevaluation() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let n0 = Network::new(network_id).await?;

	let n1 = Network::builder(network_id)
		.with_discovery(
			discovery::Config::builder() //
				.with_tags(["leader"]),
		)
		.build()
		.await?;

	let n2 = Network::builder(network_id)
		.with_discovery(
			discovery::Config::builder() //
				.with_tags(["follower"]),
		)
		.build()
		.await?;

	// Consumer that only subscribes to producers tagged "leader"
	let consumer = n0
		.streams()
		.consumer::<Data1>()
		.subscribe_if(|peer| peer.tags().contains(&"leader".into()))
		.build();

	let _p1 = n1.streams().produce::<Data1>();
	let _p2 = n2.streams().produce::<Data1>();

	discover_all([&n0, &n1, &n2]).await?;

	// Consumer should connect to n1 (has "leader" tag) but not n2
	timeout_s(3, consumer.when().subscribed()).await?;
	let subs = consumer.producers().collect::<Vec<_>>();
	assert_eq!(subs.len(), 1, "should be subscribed to exactly 1 producer");
	assert_eq!(*subs[0].peer().id(), n1.local().id());
	drop(subs);

	// Simulate n1 losing the "leader" tag by feeding an updated entry
	// into n0's catalog. Get n1's current signed entry, convert to unsigned,
	// remove the tag, re-sign, and feed.
	let n1_entry = n0
		.discovery()
		.catalog()
		.get_signed(&n1.local().id())
		.expect("n1 should be in catalog")
		.clone();
	let updated: PeerEntry = n1_entry.into();
	let updated = updated.remove_tags("leader");
	let updated = updated.sign(n1.local().secret_key())?;
	n0.discovery().feed(updated);

	// Consumer should disconnect from n1 since it no longer has the tag
	timeout_s(3, consumer.when().unsubscribed()).await?;
	let subs = consumer.producers().collect::<Vec<_>>();
	assert_eq!(subs.len(), 0, "should have no subscriptions after tag removal");
	drop(subs);

	// Simulate n2 gaining the "leader" tag
	let n2_entry = n0
		.discovery()
		.catalog()
		.get_signed(&n2.local().id())
		.expect("n2 should be in catalog")
		.clone();
	let updated: PeerEntry = n2_entry.into();
	let updated = updated.remove_tags("follower").add_tags("leader");
	let updated = updated.sign(n2.local().secret_key())?;
	n0.discovery().feed(updated);

	// Consumer should now connect to n2
	timeout_s(3, consumer.when().subscribed()).await?;
	let subs = consumer.producers().collect::<Vec<_>>();
	assert_eq!(subs.len(), 1, "should be subscribed to new leader");
	assert_eq!(*subs[0].peer().id(), n2.local().id());

	Ok(())
}
