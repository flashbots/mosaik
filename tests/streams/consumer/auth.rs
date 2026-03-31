use {
	super::*,
	crate::utils::{discover_all, sleep_s, timeout_s},
	futures::{SinkExt, StreamExt},
	hmac::{Hmac, digest::KeyInit},
	jwt::{RegisteredClaims, SignWithKey, VerifyWithKey},
	mosaik::{primitives::Short, *},
};

/// Verifies that calling `require` multiple times composes predicates with
/// AND semantics — a producer must satisfy all predicates to be subscribed to.
#[tokio::test]
async fn require_chaining() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	// Consumer node
	let n0 = Network::new(network_id).await?;

	// Producer with both required tags
	let n_both = Network::builder(network_id)
		.with_discovery(
			discovery::Config::builder().with_tags(["tag1", "tag2"]),
		)
		.build()
		.await?;

	// Producer with only the first required tag
	let n_one = Network::builder(network_id)
		.with_discovery(discovery::Config::builder().with_tags(["tag1"]))
		.build()
		.await?;

	// Producer with neither tag
	let n_none = Network::builder(network_id)
		.with_discovery(discovery::Config::builder().with_tags(["tag3"]))
		.build()
		.await?;

	let _p_both = n_both.streams().produce::<Data1>();
	let _p_one = n_one.streams().produce::<Data1>();
	let _p_none = n_none.streams().produce::<Data1>();

	// Two chained require calls — producer must have BOTH tag1 AND tag2.
	let consumer = n0
		.streams()
		.consumer::<Data1>()
		.require(|peer| peer.tags().contains(&"tag1".into()))
		.require(|peer| peer.tags().contains(&"tag2".into()))
		.build();

	discover_all([&n0, &n_both, &n_one, &n_none]).await?;

	// Only n_both satisfies both predicates.
	timeout_s(2, consumer.when().subscribed()).await?;

	// Allow a moment for n_one and n_none to attempt (and fail) subscription.
	sleep_s(2).await;

	let producers = consumer
		.producers()
		.map(|p| *p.peer().id())
		.collect::<Vec<_>>();
	assert_eq!(producers.len(), 1);
	assert!(producers.contains(&n_both.local().id()));

	Ok(())
}

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
		.require(|peer| peer.tags().contains(&"tag2".into()))
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

/// This test verifies that consumers can authenticate producers based on
/// tickets and refuse to subscribe to producers that do not present a valid
/// ticket.
#[tokio::test]
async fn by_ticket() -> anyhow::Result<()> {
	const TICKET_CLASS: UniqueId = id!("mosaik.test.jwt");
	const JWT_SECRET: &[u8] = b"some-test-secret";
	const JWT_ISSUER: &str = "mosaik.test.jwt.issuer";

	let network_id = NetworkId::random();

	let (n0, n1, n2, n3) = tokio::try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id)
	)?;

	let jwt_key: Hmac<sha2::Sha256> = Hmac::new_from_slice(JWT_SECRET).unwrap();

	// unexpired ticket
	let n1_ticket = Ticket::new(
		TICKET_CLASS,
		jwt::Claims::new(RegisteredClaims {
			issuer: Some(JWT_ISSUER.into()),
			subject: Some(n1.local().id().to_string()),
			expiration: Some(
				(chrono::Utc::now() + chrono::Duration::hours(1))
					.timestamp()
					.cast_unsigned(),
			),
			..Default::default()
		})
		.sign_with_key(&jwt_key)
		.unwrap()
		.into(),
	);

	// expired ticket
	let n2_ticket = Ticket::new(
		TICKET_CLASS,
		jwt::Claims::new(RegisteredClaims {
			issuer: Some(JWT_ISSUER.into()),
			subject: Some(n2.local().id().to_string()),
			expiration: Some(
				(chrono::Utc::now() - chrono::Duration::hours(1))
					.timestamp()
					.cast_unsigned(),
			),
			..Default::default()
		})
		.sign_with_key(&jwt_key)
		.unwrap()
		.into(),
	);

	n1.discovery().add_ticket(n1_ticket);
	n2.discovery().add_ticket(n2_ticket);

	// p1 has a valid ticket, p2 has an expired ticket, p3 has no ticket
	let mut p1 = n1.streams().produce::<Data1>();
	let _p2 = n2.streams().produce::<Data1>();
	let _p3 = n3.streams().produce::<Data1>();

	// This consumer only subscribes to producers that present a valid ticket.
	let mut c0 = n0
		.streams()
		.consumer::<Data1>()
		.require(move |peer| {
			peer.has_valid_ticket(TICKET_CLASS, |jwt_bytes| {
				let jwt_str = str::from_utf8(jwt_bytes).unwrap();
				let claims: jwt::Claims = jwt_str.verify_with_key(&jwt_key).unwrap();
				claims.registered.issuer.as_deref() == Some(JWT_ISSUER)
					&& claims.registered.subject == Some(peer.id().to_string())
					&& claims.registered.expiration
						> Some(chrono::Utc::now().timestamp().cast_unsigned())
			})
		})
		.build();

	timeout_s(5, discover_all([&n0, &n1, &n2, &n3])).await??;

	// c0 should only subscribe to p1 because it's the only producer with a
	// valid ticket. p2's ticket is expired and p3 has no ticket.
	sleep_s(5).await; // wait for subscriptions to propagate

	let producers = c0.producers().map(|p| *p.peer().id()).collect::<Vec<_>>();
	assert_eq!(producers.len(), 1);
	assert!(producers.contains(&n1.local().id()));

	p1.send(Data1("hello".into())).await?;
	assert_eq!(timeout_s(2, c0.next()).await?, Some(Data1("hello".into())));

	Ok(())
}

/// Verifies that consumers disconnect from producers whose tags change such
/// that the `require` predicate no longer matches, and reconnect to
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
		.require(|peer| peer.tags().contains(&"leader".into()))
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
	n1.discovery().remove_tags("leader");
	n0.discovery().feed(n1.discovery().me());

	// Consumer should disconnect from n1 since it no longer has the tag
	timeout_s(3, consumer.when().unsubscribed()).await?;
	let subs = consumer.producers().collect::<Vec<_>>();
	assert_eq!(
		subs.len(),
		0,
		"should have no subscriptions after tag removal"
	);
	drop(subs);

	// Simulate n2 gaining the "leader" tag
	n2.discovery().add_tags("leader");
	n0.discovery().feed(n2.discovery().me());

	// Consumer should now connect to n2
	timeout_s(3, consumer.when().subscribed()).await?;
	let subs = consumer.producers().collect::<Vec<_>>();
	assert_eq!(subs.len(), 1, "should be subscribed to new leader");
	assert_eq!(*subs[0].peer().id(), n2.local().id());

	Ok(())
}
