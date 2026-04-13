use {
	crate::{
		Data1,
		utils::{
			DEFAULT_ISSUER,
			DEFAULT_SECRET,
			discover_all,
			expired_expiry,
			expiry_in,
			jwt_builder,
			jwt_validator,
			sleep_s,
			timeout_s,
			valid_expiry,
		},
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

	let builder = jwt_builder(DEFAULT_ISSUER, DEFAULT_SECRET);

	// n1 gets a valid ticket, n2 an expired one, n3 none
	n1.discovery()
		.add_ticket(builder.build(&n1.local().id(), valid_expiry()));
	n2.discovery()
		.add_ticket(builder.build(&n2.local().id(), expired_expiry()));

	let mut p1 = n1.streams().produce::<Data1>();
	let _p2 = n2.streams().produce::<Data1>();
	let _p3 = n3.streams().produce::<Data1>();

	// Consumer with ticket validator
	let mut c0 = n0
		.streams()
		.consumer::<Data1>()
		.require_ticket(jwt_validator(DEFAULT_ISSUER, DEFAULT_SECRET))
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

	let builder = jwt_builder(DEFAULT_ISSUER, DEFAULT_SECRET);

	// Short-lived ticket (3 seconds)
	n1.discovery().add_ticket(
		builder.build(&n1.local().id(), expiry_in(Duration::from_secs(3))),
	);

	let mut p1 = n1.streams().produce::<Data1>();
	let mut c0 = n0
		.streams()
		.consumer::<Data1>()
		.require_ticket(jwt_validator(DEFAULT_ISSUER, DEFAULT_SECRET))
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

	let builder = jwt_builder(DEFAULT_ISSUER, DEFAULT_SECRET);

	let (n0, n1, n2, n3) = tokio::try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
	)?;

	// n1 gets a valid ticket, n2 an expired one, n3 none
	n1.discovery()
		.add_ticket(builder.build(&n1.local().id(), valid_expiry()));
	n2.discovery()
		.add_ticket(builder.build(&n2.local().id(), expired_expiry()));

	// Producer with ticket validator
	let mut p0 = n0
		.streams()
		.producer::<Data1>()
		.require_ticket(jwt_validator(DEFAULT_ISSUER, DEFAULT_SECRET))
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

	let builder_a = jwt_builder("issuer-alpha", "secret-alpha");
	let builder_b = jwt_builder("issuer-beta", "secret-beta");

	let (n0, n1, n2, n3) = tokio::try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
	)?;

	// n1 has both tickets → should be subscribed
	n1.discovery()
		.add_ticket(builder_a.build(&n1.local().id(), valid_expiry()));
	n1.discovery()
		.add_ticket(builder_b.build(&n1.local().id(), valid_expiry()));

	// n2 has only issuer_a ticket → should be rejected
	n2.discovery()
		.add_ticket(builder_a.build(&n2.local().id(), valid_expiry()));

	// n3 has only issuer_b ticket → should be rejected
	n3.discovery()
		.add_ticket(builder_b.build(&n3.local().id(), valid_expiry()));

	let mut p1 = n1.streams().produce::<Data1>();
	let _p2 = n2.streams().produce::<Data1>();
	let _p3 = n3.streams().produce::<Data1>();

	// Consumer requiring both validators
	let mut c0 = n0
		.streams()
		.consumer::<Data1>()
		.require_ticket(jwt_validator("issuer-alpha", "secret-alpha"))
		.require_ticket(jwt_validator("issuer-beta", "secret-beta"))
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

	let builder_a = jwt_builder("issuer-alpha", "secret-alpha");
	let builder_b = jwt_builder("issuer-beta", "secret-beta");

	let (n0, n1, n2, n3) = tokio::try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
	)?;

	// n1 has both tickets → should be accepted
	n1.discovery()
		.add_ticket(builder_a.build(&n1.local().id(), valid_expiry()));
	n1.discovery()
		.add_ticket(builder_b.build(&n1.local().id(), valid_expiry()));

	// n2 has only issuer_a ticket → should be rejected
	n2.discovery()
		.add_ticket(builder_a.build(&n2.local().id(), valid_expiry()));

	// n3 has only issuer_b ticket → should be rejected
	n3.discovery()
		.add_ticket(builder_b.build(&n3.local().id(), valid_expiry()));

	// Producer requiring both validators
	let mut p0 = n0
		.streams()
		.producer::<Data1>()
		.require_ticket(jwt_validator("issuer-alpha", "secret-alpha"))
		.require_ticket(jwt_validator("issuer-beta", "secret-beta"))
		.build()?;

	let mut c1 = n1.streams().consume::<Data1>();
	let c2 = n2.streams().consume::<Data1>();
	let c3 = n3.streams().consume::<Data1>();

	timeout_s(5, discover_all([&n0, &n1, &n2, &n3])).await??;
	sleep_s(5).await;

	let subscribers = p0.consumers().map(|s| *s.peer().id()).collect::<Vec<_>>();
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

	let builder = jwt_builder(DEFAULT_ISSUER, DEFAULT_SECRET);

	// Short-lived ticket (3 seconds)
	n1.discovery().add_ticket(
		builder.build(&n1.local().id(), expiry_in(Duration::from_secs(3))),
	);

	let mut p0 = n0
		.streams()
		.producer::<Data1>()
		.require_ticket(jwt_validator(DEFAULT_ISSUER, DEFAULT_SECRET))
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

/// Verifies end-to-end ES256 (P-256 ECDSA) JWT authentication: tickets
/// signed with a private key are accepted by a validator configured with
/// the corresponding compressed public key, and tickets signed with a
/// different key are rejected.
#[tokio::test]
async fn es256_asymmetric_tickets() -> anyhow::Result<()> {
	use mosaik::tickets::{Es256SigningKey, Jwt, JwtTicketBuilder};

	let network_id = NetworkId::random();

	// Generate a random P-256 keypair.
	let signing_key = Es256SigningKey::random();
	let pubkey = signing_key.verifying_key();

	// Build tickets with the private key, validate with the public key.
	let builder = JwtTicketBuilder::new(signing_key).issuer("es256-issuer");
	let validator = Jwt::with_key(pubkey).allow_issuer("es256-issuer");

	let (n0, n1, n2) = tokio::try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
	)?;

	// n1 gets a valid ES256 ticket; n2 gets none
	n1.discovery()
		.add_ticket(builder.build(&n1.local().id(), valid_expiry()));

	let mut p1 = n1.streams().produce::<Data1>();
	let _p2 = n2.streams().produce::<Data1>();

	let mut c0 = n0
		.streams()
		.consumer::<Data1>()
		.require_ticket(validator)
		.build();

	timeout_s(5, discover_all([&n0, &n1, &n2])).await??;
	sleep_s(5).await;

	// Only n1 should be subscribed (valid ES256 ticket)
	let producers = c0.producers().map(|p| *p.peer().id()).collect::<Vec<_>>();
	assert_eq!(producers.len(), 1, "only n1 has a valid ES256 ticket");
	assert!(producers.contains(&n1.local().id()));

	// Data flows through the authenticated connection
	p1.send(Data1("es256".into())).await?;
	assert_eq!(timeout_s(2, c0.next()).await?, Some(Data1("es256".into())));

	Ok(())
}

/// Verifies ES256 with a compile-time hex-encoded public key and a
/// fixed private key, demonstrating that the validator can be declared
/// as a constant expression.
#[tokio::test]
async fn es256_hex_const_key() -> anyhow::Result<()> {
	use mosaik::tickets::{Es256, Es256SigningKey, Jwt, JwtTicketBuilder};

	// Fixed P-256 keypair (openssl ecparam -genkey -name prime256v1).
	const PRIVKEY: [u8; 32] = [
		0xde, 0x01, 0x77, 0x68, 0x91, 0x47, 0x6e, 0xe7, 0xbc, 0xd6, 0xee, 0x9d,
		0x98, 0x24, 0xd1, 0x44, 0x0c, 0x7a, 0xe0, 0xc0, 0x95, 0xad, 0x71, 0xad,
		0x75, 0xd3, 0xd1, 0x1a, 0x96, 0x64, 0x7b, 0x85,
	];

	let network_id = NetworkId::random();
	let signing_key = Es256SigningKey::from_bytes(PRIVKEY);

	let builder = JwtTicketBuilder::new(signing_key).issuer("es256-hex-issuer");

	let validator = Jwt::with_key(Es256::hex(
		"0298a82ebe69ad57e0f7d5c2809a05188ff58572c5a5009015f26643f91e0d3236",
	))
	.allow_issuer("es256-hex-issuer");

	let (n0, n1, n2) = tokio::try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
	)?;

	// n1 gets a valid ES256 ticket; n2 gets none
	n1.discovery()
		.add_ticket(builder.build(&n1.local().id(), valid_expiry()));

	let mut p1 = n1.streams().produce::<Data1>();
	let _p2 = n2.streams().produce::<Data1>();

	let mut c0 = n0
		.streams()
		.consumer::<Data1>()
		.require_ticket(validator)
		.build();

	timeout_s(5, discover_all([&n0, &n1, &n2])).await??;
	sleep_s(5).await;

	// Only n1 should be subscribed
	let producers = c0.producers().map(|p| *p.peer().id()).collect::<Vec<_>>();
	assert_eq!(producers.len(), 1, "only n1 has a valid ES256 ticket");
	assert!(producers.contains(&n1.local().id()));

	// Data flows through the authenticated connection
	p1.send(Data1("es256-hex".into())).await?;
	assert_eq!(
		timeout_s(2, c0.next()).await?,
		Some(Data1("es256-hex".into()))
	);

	Ok(())
}

/// Verifies that the `stream!` macro can declare a stream with an ES256
/// `require_ticket` validator using a compile-time hex public key, and
/// that only producers with a matching ticket are accepted.
#[tokio::test]
async fn es256_stream_macro_hex_key() -> anyhow::Result<()> {
	use mosaik::tickets::{Es256, Es256SigningKey, Jwt, JwtTicketBuilder};

	// Same fixed keypair as es256_hex_const_key.
	const PRIVKEY: [u8; 32] = [
		0xde, 0x01, 0x77, 0x68, 0x91, 0x47, 0x6e, 0xe7, 0xbc, 0xd6, 0xee, 0x9d,
		0x98, 0x24, 0xd1, 0x44, 0x0c, 0x7a, 0xe0, 0xc0, 0x95, 0xad, 0x71, 0xad,
		0x75, 0xd3, 0xd1, 0x1a, 0x96, 0x64, 0x7b, 0x85,
	];

	// Declare a stream where producers are required to present a valid ES256
	// ticket with the specified public key.
	declare::stream!(
		Es256AuthStream = Data1, "es256.auth.stream",
		producer require_ticket: Jwt::with_key(Es256::hex(
				"0298a82ebe69ad57e0f7d5c2809a05188ff58572c5a5009015f26643f91e0d3236"
			)).allow_issuer("es256-macro-issuer"),
	);

	let network_id = NetworkId::random();

	let (n0, n1, n2) = tokio::try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
	)?;

	// n0 (producer) gets a valid ES256 ticket; n2 (producer) gets none.
	n0.discovery().add_ticket(
		JwtTicketBuilder::new(Es256SigningKey::from_bytes(PRIVKEY))
			.issuer("es256-macro-issuer")
			.build(&n0.local().id(), valid_expiry()),
	);

	let mut p0 = Es256AuthStream::producer(&n0);
	let _p2 = Es256AuthStream::producer(&n2);
	let mut c1 = Es256AuthStream::consumer(&n1);

	timeout_s(5, discover_all([&n0, &n1, &n2])).await??;

	sleep_s(5).await;

	// c1 should only be subscribed to n0 (valid ticket)
	let producers = c1.producers().map(|p| *p.peer().id()).collect::<Vec<_>>();
	assert_eq!(producers.len(), 1, "only n0 has a valid ES256 ticket");
	assert!(producers.contains(&n0.local().id()));

	// Data flows through the authenticated connection
	p0.send(Data1("es256-macro".into())).await?;
	assert_eq!(
		timeout_s(2, c1.next()).await?,
		Some(Data1("es256-macro".into()))
	);

	Ok(())
}
