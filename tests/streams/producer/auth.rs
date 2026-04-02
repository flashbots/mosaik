use {
	super::*,
	crate::utils::{JwtIssuer, discover_all, sleep_s, timeout_s},
	core::time::Duration,
	futures::{SinkExt, StreamExt},
	hmac::{Hmac, digest::KeyInit},
	jwt::{RegisteredClaims, SignWithKey, VerifyWithKey},
	mosaik::{tickets::jwt::JwtTicketValidator, *},
};

/// Verifies that calling `require` multiple times composes predicates with
/// AND semantics — a consumer must satisfy all predicates to be accepted.
#[tokio::test]
async fn require_chaining() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	// Producer node
	let n0 = Network::new(network_id).await?;

	// Consumer with both required tags
	let n_both = Network::builder(network_id)
		.with_discovery(discovery::Config::builder().with_tags(["tag1", "tag2"]))
		.build()
		.await?;

	// Consumer with only the first required tag
	let n_one = Network::builder(network_id)
		.with_discovery(discovery::Config::builder().with_tags(["tag1"]))
		.build()
		.await?;

	// Consumer with neither tag
	let n_none = Network::builder(network_id)
		.with_discovery(discovery::Config::builder().with_tags(["tag3"]))
		.build()
		.await?;

	// Two chained require calls — consumer must have BOTH tag1 AND tag2.
	let p0 = n0
		.streams()
		.producer::<Data1>()
		.require(|peer| peer.tags().contains(&"tag1".into()))
		.require(|peer| peer.tags().contains(&"tag2".into()))
		.build()?;

	let c_both = n_both.streams().consume::<Data1>();
	let c_one = n_one.streams().consume::<Data1>();
	let c_none = n_none.streams().consume::<Data1>();

	discover_all([&n0, &n_both, &n_one, &n_none]).await?;

	// Only c_both should be subscribed to p0; the others fail at least one
	// predicate.
	timeout_s(2, c_both.when().subscribed()).await?;
	timeout_s(2, p0.when().subscribed().minimum_of(1)).await?;

	// Allow a moment for c_one and c_none to attempt (and fail) subscription.
	sleep_s(2).await;

	let subscribers = p0.consumers().map(|s| *s.peer().id()).collect::<Vec<_>>();
	assert_eq!(subscribers.len(), 1);
	assert!(subscribers.contains(&n_both.local().id()));
	assert_eq!(c_one.producers().count(), 0);
	assert_eq!(c_none.producers().count(), 0);

	Ok(())
}

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
		.require(|peer| peer.tags().contains(&"tag2".into()))
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
async fn by_jwt_ticket() -> anyhow::Result<()> {
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

	timeout_s(5, discover_all([&n0, &n1, &n2, &n3])).await??;

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

/// Producer uses `with_ticket_validator` to only accept consumers
/// that present a valid JWT ticket: valid ticket → accepted,
/// expired ticket or no ticket → rejected.
#[tokio::test]
async fn with_jwt_ticket_validator() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let jwt_issuer = JwtIssuer::default();

	let (n0, n1, n2, n3) = tokio::try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
	)?;

	// n1 gets a valid ticket, n2 an expired one, n3 none
	let n1_ticket = jwt_issuer.make_valid_ticket(&n1.local().id());
	let n2_ticket = jwt_issuer.make_expired_ticket(&n2.local().id());

	n1.discovery().add_ticket(n1_ticket);
	n2.discovery().add_ticket(n2_ticket);

	// Producer with ticket validator
	let mut p0 = n0
		.streams()
		.producer::<Data1>()
		.with_ticket_validator(
			JwtTicketValidator::with_key(jwt_issuer.key())
				.allow_issuer(jwt_issuer.issuer()),
		)
		.build()?;

	let mut c1 = n1.streams().consume::<Data1>();
	let c2 = n2.streams().consume::<Data1>();
	let c3 = n3.streams().consume::<Data1>();

	timeout_s(5, discover_all([&n0, &n1, &n2, &n3])).await??;

	// Only n1 should be accepted
	sleep_s(5).await;

	let subscribers = p0.consumers().map(|s| *s.peer().id()).collect::<Vec<_>>();
	assert_eq!(subscribers.len(), 1);
	assert!(subscribers.contains(&n1.local().id()));

	// Data flows to the valid subscriber
	p0.send(Data1("hello".into())).await?;
	assert_eq!(timeout_s(2, c1.next()).await?, Some(Data1("hello".into())));

	assert_eq!(c2.producers().count(), 0);
	assert_eq!(c3.producers().count(), 0);

	Ok(())
}

/// Producer disconnects a consumer whose ticket expires mid-session.
#[tokio::test]
async fn with_jwt_ticket_validator_expiry() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let (n0, n1) =
		tokio::try_join!(Network::new(network_id), Network::new(network_id),)?;

	let jwt_issuer = JwtIssuer::default();

	// Short-lived ticket (3 seconds)
	let n1_ticket = jwt_issuer
		.make_ticket_expiring_in(&n1.local().id(), Duration::from_secs(3));
	n1.discovery().add_ticket(n1_ticket);

	let mut p0 = n0
		.streams()
		.producer::<Data1>()
		.with_ticket_validator(
			JwtTicketValidator::with_key(jwt_issuer.key())
				.allow_issuer(jwt_issuer.issuer()),
		)
		.build()?;

	let mut c1 = n1.streams().consume::<Data1>();

	timeout_s(5, discover_all([&n0, &n1])).await??;
	timeout_s(3, p0.when().subscribed()).await?;

	// Data works while the ticket is valid
	p0.send(Data1("before-expiry".into())).await?;
	assert_eq!(
		timeout_s(2, c1.next()).await?,
		Some(Data1("before-expiry".into()))
	);

	// Wait for the ticket to expire
	sleep_s(5).await;

	// Producer should have dropped the subscriber
	assert_eq!(
		p0.consumers().count(),
		0,
		"producer should drop consumer after ticket expiry"
	);

	Ok(())
}
