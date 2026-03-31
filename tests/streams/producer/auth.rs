use {
	super::*,
	crate::utils::{discover_all, sleep_s, timeout_s},
	futures::{SinkExt, StreamExt},
	hmac::{Hmac, digest::KeyInit},
	jwt::{RegisteredClaims, SignWithKey, VerifyWithKey},
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

#[tokio::test]
async fn by_ticket() -> anyhow::Result<()> {
	const TICKET_CLASS: UniqueId = unique_id!("mosaik.test.jwt");
	const JWT_SECRET: &[u8] = b"some-test-secret";
	const JWT_ISSUER: &str = "mosaik.test.jwt.issuer";

	let network_id = NetworkId::random();

	let (n0, n1, n2, n3) = tokio::try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id)
	)?;

	timeout_s(5, discover_all([&n0, &n1, &n2])).await??;

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

	let mut p0 = n0
		.streams()
		.producer::<Data1>()
		.accept_if(move |peer| {
			peer.has_valid_ticket(TICKET_CLASS, |jwt_bytes| {
				let jwt_str = str::from_utf8(jwt_bytes).unwrap();
				let claims: jwt::Claims = jwt_str.verify_with_key(&jwt_key).unwrap();
				claims.registered.issuer.as_deref() == Some(JWT_ISSUER)
					&& claims.registered.subject == Some(peer.id().to_string())
					&& claims.registered.expiration
						> Some(chrono::Utc::now().timestamp().cast_unsigned())
			})
		})
		.build()?;

	let mut c1 = n1.streams().consume::<Data1>();
	let c2 = n2.streams().consume::<Data1>();
	let c3 = n3.streams().consume::<Data1>();

	// c1 should subscribe to p0 because it has a valid ticket, while c2 should
	// not subscribe because its ticket is expired. c3 should not subscribe
	// because it has no ticket.
	sleep_s(5).await; // wait for subscriptions to propagate

	let subscribers = p0.consumers().map(|s| *s.peer().id()).collect::<Vec<_>>();
	assert_eq!(subscribers.len(), 1);
	assert!(subscribers.contains(&n1.local().id()));

	p0.send(Data1("one".into())).await?;
	assert_eq!(timeout_s(2, c1.next()).await?, Some(Data1("one".into())));

	assert_eq!(c1.producers().count(), 1);
	assert_eq!(c2.producers().count(), 0);
	assert_eq!(c3.producers().count(), 0);

	Ok(())
}
