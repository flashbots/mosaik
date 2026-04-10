use {
	crate::{
		Data1,
		utils::{JwtIssuer, JwtTicketValidator, discover_all, sleep_s, timeout_s},
	},
	core::time::Duration,
	futures::{SinkExt, StreamExt},
	mosaik::*,
};

/// Consumer uses `with_ticket_validator` to only subscribe to producers
/// that present a valid JWT ticket: valid ticket → subscribed,
/// expired ticket or no ticket → rejected.
#[tokio::test]
async fn stream_consumer() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let (n0, n1, n2, n3) = tokio::try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
	)?;

	let jwt_issuer = JwtIssuer::default();

	// n1 gets a valid ticket, n2 an expired one, n3 none
	let n1_ticket = jwt_issuer.make_valid_ticket(&n1.local().id());
	let n2_ticket = jwt_issuer.make_expired_ticket(&n2.local().id());

	n1.discovery().add_ticket(n1_ticket);
	n2.discovery().add_ticket(n2_ticket);

	let mut p1 = n1.streams().produce::<Data1>();
	let _p2 = n2.streams().produce::<Data1>();
	let _p3 = n3.streams().produce::<Data1>();

	// Consumer with ticket validator
	let mut c0 = n0
		.streams()
		.consumer::<Data1>()
		.with_ticket_validator(
			JwtTicketValidator::with_key(jwt_issuer.key())
				.allow_issuer(jwt_issuer.issuer()),
		)
		.build();

	timeout_s(5, discover_all([&n0, &n1, &n2, &n3])).await??;

	// Only n1 should be subscribed
	sleep_s(5).await;

	let producers = c0.producers().map(|p| *p.peer().id()).collect::<Vec<_>>();
	assert_eq!(producers.len(), 1);
	assert!(producers.contains(&n1.local().id()));

	// Data flows through the valid connection
	p1.send(Data1("hello".into())).await?;
	assert_eq!(timeout_s(2, c0.next()).await?, Some(Data1("hello".into())));

	Ok(())
}

/// Consumer disconnects from a producer whose ticket expires mid-session.
#[tokio::test]
async fn stream_consumer_ticket_expiry() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let (n0, n1) =
		tokio::try_join!(Network::new(network_id), Network::new(network_id),)?;

	let jwt_issuer = JwtIssuer::default();

	// Short-lived ticket (3 seconds)
	let n1_ticket = jwt_issuer
		.make_ticket_expiring_in(&n1.local().id(), Duration::from_secs(3));
	n1.discovery().add_ticket(n1_ticket);

	let mut p1 = n1.streams().produce::<Data1>();
	let mut c0 = n0
		.streams()
		.consumer::<Data1>()
		.with_ticket_validator(
			JwtTicketValidator::with_key(jwt_issuer.key())
				.allow_issuer(jwt_issuer.issuer()),
		)
		.build();

	timeout_s(5, discover_all([&n0, &n1])).await??;
	timeout_s(3, c0.when().subscribed()).await?;

	// Data works while the ticket is valid
	p1.send(Data1("before-expiry".into())).await?;
	assert_eq!(
		timeout_s(2, c0.next()).await?,
		Some(Data1("before-expiry".into()))
	);

	// Wait for the ticket to expire
	sleep_s(5).await;

	// Consumer should have disconnected
	assert_eq!(
		c0.producers().count(),
		0,
		"consumer should disconnect after ticket expiry"
	);

	Ok(())
}

/// Producer uses `with_ticket_validator` to only accept consumers
/// that present a valid JWT ticket: valid ticket → accepted,
/// expired ticket or no ticket → rejected.
#[tokio::test]
async fn stream_producer() -> anyhow::Result<()> {
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

/// Consumer with multiple ticket validators only subscribes to producers that
/// satisfy **all** of them. Producers with only a subset of the required
/// tickets are skipped.
#[tokio::test]
async fn stream_consumer_multiple_validators() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let issuer_a = JwtIssuer::new("issuer-alpha", "secret-alpha");
	let issuer_b = JwtIssuer::new("issuer-beta", "secret-beta");

	let (n0, n1, n2, n3) = tokio::try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
	)?;

	// n1 has both tickets → should be subscribed
	n1.discovery()
		.add_ticket(issuer_a.make_valid_ticket(&n1.local().id()));
	n1.discovery()
		.add_ticket(issuer_b.make_valid_ticket(&n1.local().id()));

	// n2 has only issuer_a ticket → should be rejected
	n2.discovery()
		.add_ticket(issuer_a.make_valid_ticket(&n2.local().id()));

	// n3 has only issuer_b ticket → should be rejected
	n3.discovery()
		.add_ticket(issuer_b.make_valid_ticket(&n3.local().id()));

	let mut p1 = n1.streams().produce::<Data1>();
	let _p2 = n2.streams().produce::<Data1>();
	let _p3 = n3.streams().produce::<Data1>();

	// Consumer requiring both validators
	let mut c0 = n0
		.streams()
		.consumer::<Data1>()
		.with_ticket_validator(
			JwtTicketValidator::with_key(issuer_a.key())
				.allow_issuer(issuer_a.issuer()),
		)
		.with_ticket_validator(
			JwtTicketValidator::with_key(issuer_b.key())
				.allow_issuer(issuer_b.issuer()),
		)
		.build();

	timeout_s(5, discover_all([&n0, &n1, &n2, &n3])).await??;
	sleep_s(5).await;

	let producers = c0.producers().map(|p| *p.peer().id()).collect::<Vec<_>>();
	assert_eq!(producers.len(), 1, "only n1 satisfies both validators");
	assert!(producers.contains(&n1.local().id()));

	// Data flows through the valid connection
	p1.send(Data1("multi-auth".into())).await?;
	assert_eq!(
		timeout_s(2, c0.next()).await?,
		Some(Data1("multi-auth".into()))
	);

	Ok(())
}

/// Producer with multiple ticket validators only accepts consumers that
/// satisfy **all** of them. Consumers with only a subset of the required
/// tickets are rejected.
#[tokio::test]
async fn stream_producer_multiple_validators() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let issuer_a = JwtIssuer::new("issuer-alpha", "secret-alpha");
	let issuer_b = JwtIssuer::new("issuer-beta", "secret-beta");

	let (n0, n1, n2, n3) = tokio::try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
	)?;

	// n1 has both tickets → should be accepted
	n1.discovery()
		.add_ticket(issuer_a.make_valid_ticket(&n1.local().id()));
	n1.discovery()
		.add_ticket(issuer_b.make_valid_ticket(&n1.local().id()));

	// n2 has only issuer_a ticket → should be rejected
	n2.discovery()
		.add_ticket(issuer_a.make_valid_ticket(&n2.local().id()));

	// n3 has only issuer_b ticket → should be rejected
	n3.discovery()
		.add_ticket(issuer_b.make_valid_ticket(&n3.local().id()));

	// Producer requiring both validators
	let mut p0 = n0
		.streams()
		.producer::<Data1>()
		.with_ticket_validator(
			JwtTicketValidator::with_key(issuer_a.key())
				.allow_issuer(issuer_a.issuer()),
		)
		.with_ticket_validator(
			JwtTicketValidator::with_key(issuer_b.key())
				.allow_issuer(issuer_b.issuer()),
		)
		.build()?;

	let mut c1 = n1.streams().consume::<Data1>();
	let c2 = n2.streams().consume::<Data1>();
	let c3 = n3.streams().consume::<Data1>();

	timeout_s(5, discover_all([&n0, &n1, &n2, &n3])).await??;
	sleep_s(5).await;

	let subscribers =
		p0.consumers().map(|s| *s.peer().id()).collect::<Vec<_>>();
	assert_eq!(subscribers.len(), 1, "only n1 satisfies both validators");
	assert!(subscribers.contains(&n1.local().id()));

	// Data flows to the valid subscriber
	p0.send(Data1("multi-auth".into())).await?;
	assert_eq!(
		timeout_s(2, c1.next()).await?,
		Some(Data1("multi-auth".into()))
	);

	assert_eq!(c2.producers().count(), 0);
	assert_eq!(c3.producers().count(), 0);

	Ok(())
}

/// Producer disconnects a consumer whose ticket expires mid-session.
#[tokio::test]
async fn stream_producer_ticket_expiry() -> anyhow::Result<()> {
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
