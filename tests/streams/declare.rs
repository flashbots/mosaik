use {
	super::*,
	crate::utils::{Jwt, JwtIssuer, discover_all, sleep_s, timeout_s},
	digest::KeyInit,
	futures::{SinkExt, StreamExt},
	hmac::Hmac,
	mosaik::{declare, *},
	serde::{Deserialize, Serialize},
};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct Data2(String);

// Simplest form: type-derived StreamId, both producer and consumer.
declare::stream!(pub(crate) SimpleStream = Data1);

// With explicit StreamId.
declare::stream!(NamedStream = Data1, "named.stream");

// Producer only with require config.
declare::stream!(
	pub(crate) producer ProducerOnlyStream = Data1,
		"producer.only",
		require: |_peer| true,
);

// Consumer only with require config.
declare::stream!(
	pub(crate) consumer ConsumerOnlyStream = Data1,
		"consumer.only",
		require: |_peer| true,
);

// Full with side-prefixed online_when.
declare::stream!(
	pub(crate) ConfiguredStream = Data1,
		"configured.stream",
		require: |_peer| true,
		producer online_when: |c| c.minimum_of(1),
);

// Producer-only stream that requires consumers to carry the "trusted" tag.
declare::stream!(
	pub producer TaggedProducerStream = Data2,
		"tagged.producer.stream",
		require: |peer| peer.tags().contains(&"trusted".into()),
);

// Consumer-only stream that only subscribes to producers with the "trusted"
// tag.
declare::stream!(
	pub consumer TaggedConsumerStream = Data2,
		"tagged.consumer.stream",
		require: |peer| peer.tags().contains(&"trusted".into()),
);

declare::stream!(
	/// A well-known price feed.
	pub(crate) PriceFeed = Data2, "oracle.price"
);

// With a constant stream id expression.
const CONST_STREAM_ID: StreamId = unique_id!("const.stream");
declare::stream!(pub(crate) ConstIdStream = Data1, CONST_STREAM_ID);

// Constant stream id with config entries.
declare::stream!(
	pub(crate) ConstIdConfiguredStream = Data1, CONST_STREAM_ID,
		require: |_peer| true,
		producer online_when: |c| c.minimum_of(1),
);

// Both sides use the same ticket validator.
declare::stream!(
	pub(crate) TicketAuthStream = Data1,
		"ticket.auth.stream",
		require_ticket: Jwt::with_key(
				JwtIssuer::default().key()
			),
);

// Producer-only with ticket validator.
declare::stream!(
	pub(crate) producer ProducerTicketStream = Data1,
		"producer.ticket.stream",
		require_ticket: {
			let issuer = JwtIssuer::default();
			Jwt::with_key(issuer.key()).allow_issuer(issuer.issuer())
		},
);

// Consumer-only with ticket validator.
declare::stream!(
	pub(crate) consumer ConsumerTicketStream = Data1,
		"consumer.ticket.stream",
		require_ticket: Jwt::with_key(JwtIssuer::default().key()),
);

// Both sides require two independent JWT validators (different issuers).
declare::stream!(
	pub(crate) MultiTicketStream = Data1,
		"multi.ticket.stream",
		require_ticket: Jwt::with_key(
				Hmac::<sha2::Sha256>::new_from_slice(b"secret-alpha").unwrap()
			).allow_issuer("issuer-alpha"),
		require_ticket: Jwt::with_key(
				Hmac::<sha2::Sha256>::new_from_slice(b"secret-beta").unwrap()
			).allow_issuer("issuer-beta"),
);

#[tokio::test]
async fn stream_macro_simple() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;

	let mut p0 = SimpleStream::producer(&n0);
	let mut c1 = SimpleStream::consumer(&n1);

	discover_all([&n0, &n1]).await?;
	timeout_s(3, c1.when().subscribed()).await?;

	timeout_s(3, p0.send(Data1("hello macro".into()))).await??;

	let msg = timeout_s(3, c1.next()).await?.expect("expected message");
	assert_eq!(msg, Data1("hello macro".into()));

	Ok(())
}

#[tokio::test]
async fn stream_macro_named() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;

	let mut p0 = NamedStream::producer(&n0);
	let mut c1 = NamedStream::consumer(&n1);

	assert_eq!(p0.stream_id(), unique_id!("named.stream"));
	assert_eq!(c1.stream_id(), unique_id!("named.stream"));

	discover_all([&n0, &n1]).await?;
	timeout_s(3, c1.when().subscribed()).await?;

	timeout_s(3, p0.send(Data1("hello named".into()))).await??;

	let msg = timeout_s(3, c1.next()).await?.expect("expected message");
	assert_eq!(msg, Data1("hello named".into()));

	Ok(())
}

#[tokio::test]
async fn stream_macro_configured() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;

	let mut p0 = ConfiguredStream::producer(&n0);
	let mut c1 = ConfiguredStream::consumer(&n1);

	discover_all([&n0, &n1]).await?;
	timeout_s(3, c1.when().subscribed()).await?;

	timeout_s(3, p0.send(Data1("hello configured".into()))).await??;

	let msg = timeout_s(3, c1.next()).await?.expect("expected message");
	assert_eq!(msg, Data1("hello configured".into()));

	Ok(())
}

#[tokio::test]
async fn stream_macro_producer_only() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;

	// ProducerOnlyStream only has StreamProducer, use manual consumer.
	let mut p0 = ProducerOnlyStream::producer(&n0);
	let mut c1 = n1
		.streams()
		.consumer::<Data1>()
		.with_stream_id("producer.only")
		.build();

	discover_all([&n0, &n1]).await?;
	timeout_s(3, c1.when().subscribed()).await?;

	timeout_s(3, p0.send(Data1("hello producer only".into()))).await??;

	let msg = timeout_s(3, c1.next()).await?.expect("expected message");
	assert_eq!(msg, Data1("hello producer only".into()));

	Ok(())
}

/// Verifies that a producer-only `stream!` declaration with a `require`
/// predicate rejects consumers that do not satisfy it and accepts those that
/// do.
#[tokio::test]
async fn stream_macro_producer_require() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	// n0 hosts the producer.
	let n0 = Network::new(network_id).await?;

	// n1 has the "trusted" tag — the producer's require should accept it.
	let n1 = Network::builder(network_id)
		.with_discovery(discovery::Config::builder().with_tags("trusted"))
		.build()
		.await?;

	// n2 has no tags — the producer's require should reject it.
	let n2 = Network::new(network_id).await?;

	// TaggedProducerStream is producer-only with require: needs "trusted" tag.
	let mut p0 = TaggedProducerStream::producer(&n0);

	let mut c1 = n1
		.streams()
		.consumer::<Data2>()
		.with_stream_id("tagged.producer.stream")
		.build();

	let c2 = n2
		.streams()
		.consumer::<Data2>()
		.with_stream_id("tagged.producer.stream")
		.build();

	discover_all([&n0, &n1, &n2]).await?;

	// Trusted consumer should be accepted and connected.
	timeout_s(3, c1.when().subscribed()).await?;

	timeout_s(3, p0.send(Data2("hello trusted consumer".into()))).await??;
	let msg = timeout_s(3, c1.next())
		.await?
		.expect("expected message from trusted consumer");
	assert_eq!(msg, Data2("hello trusted consumer".into()));

	// Untrusted consumer should be rejected. Give it time to attempt and fail.
	sleep_s(2).await;
	assert!(
		!c2.when().subscribed().is_condition_met(),
		"untrusted consumer should not be subscribed"
	);

	Ok(())
}

/// Verifies that a consumer-only `stream!` declaration with a `require`
/// predicate subscribes only to producers that satisfy it and ignores others.
#[tokio::test]
async fn stream_macro_consumer_require() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	// n0 hosts the consumer.
	let n0 = Network::new(network_id).await?;

	// n1 has the "trusted" tag — the consumer's require should subscribe to it.
	let n1 = Network::builder(network_id)
		.with_discovery(discovery::Config::builder().with_tags("trusted"))
		.build()
		.await?;

	// n2 has no tags — the consumer's require should ignore it.
	let n2 = Network::new(network_id).await?;

	// TaggedConsumerStream is consumer-only with require: needs "trusted" tag.
	let mut c0 = TaggedConsumerStream::consumer(&n0);

	let mut p1 = n1
		.streams()
		.producer::<Data2>()
		.with_stream_id("tagged.consumer.stream")
		.build()?;

	let _p2 = n2
		.streams()
		.producer::<Data2>()
		.with_stream_id("tagged.consumer.stream")
		.build()?;

	discover_all([&n0, &n1, &n2]).await?;

	// Consumer should connect to n1 only.
	timeout_s(3, c0.when().subscribed()).await?;

	// Give n2 time to be discovered and filtered out.
	sleep_s(2).await;

	let producers = c0.producers().map(|p| *p.peer().id()).collect::<Vec<_>>();
	assert_eq!(
		producers.len(),
		1,
		"should be subscribed to exactly 1 producer"
	);
	assert!(
		producers.contains(&n1.local().id()),
		"should be subscribed to the trusted producer only"
	);

	// Verify data flows from the one accepted producer.
	timeout_s(3, p1.send(Data2("hello from trusted producer".into()))).await??;
	let msg = timeout_s(3, c0.next()).await?.expect("expected message");
	assert_eq!(msg, Data2("hello from trusted producer".into()));

	Ok(())
}

#[tokio::test]
async fn stream_macro_const_id() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;

	let mut p0 = ConstIdStream::producer(&n0);
	let mut c1 = ConstIdStream::consumer(&n1);

	assert_eq!(p0.stream_id(), &CONST_STREAM_ID);
	assert_eq!(c1.stream_id(), &CONST_STREAM_ID);

	discover_all([&n0, &n1]).await?;
	timeout_s(3, c1.when().subscribed()).await?;

	timeout_s(3, p0.send(Data1("hello const id".into()))).await??;

	let msg = timeout_s(3, c1.next()).await?.expect("expected message");
	assert_eq!(msg, Data1("hello const id".into()));

	Ok(())
}

/// Verifies that a `stream!` declaration with `require_ticket` gates both
/// producer and consumer sides: only peers with valid tickets are connected.
#[tokio::test]
async fn stream_macro_require_ticket() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let (n0, n1, n2, n3) = tokio::try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
	)?;

	let jwt_validator = JwtIssuer::default();

	// n0 and n1 get valid tickets; n2 gets an expired one; n3 gets none.
	n0.discovery()
		.add_ticket(jwt_validator.make_valid_ticket(&n0.local().id()));
	n1.discovery()
		.add_ticket(jwt_validator.make_valid_ticket(&n1.local().id()));
	n2.discovery()
		.add_ticket(jwt_validator.make_expired_ticket(&n2.local().id()));

	let mut p0 = TicketAuthStream::producer(&n0);
	let mut c1 = TicketAuthStream::consumer(&n1);
	let c2 = TicketAuthStream::consumer(&n2);
	let c3 = TicketAuthStream::consumer(&n3);

	timeout_s(5, discover_all([&n0, &n1, &n2, &n3])).await??;

	// c1 should connect (valid ticket on both sides).
	timeout_s(3, c1.when().subscribed()).await?;

	// c2 and c3 should not connect.
	sleep_s(3).await;
	assert!(
		!c2.when().subscribed().is_condition_met(),
		"expired-ticket consumer should not be subscribed"
	);
	assert!(
		!c3.when().subscribed().is_condition_met(),
		"no-ticket consumer should not be subscribed"
	);

	// Data flows between n0 and n1.
	p0.send(Data1("ticket-gated".into())).await?;
	let msg = timeout_s(3, c1.next()).await?.expect("expected message");
	assert_eq!(msg, Data1("ticket-gated".into()));

	Ok(())
}

/// Verifies that a producer-only `stream!` with `require_ticket` rejects
/// consumers without valid tickets.
#[tokio::test]
async fn stream_macro_producer_require_ticket() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let (n0, n1, n2) = tokio::try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
	)?;

	let jwt_validator = JwtIssuer::default();

	n1.discovery()
		.add_ticket(jwt_validator.make_valid_ticket(&n1.local().id()));

	let mut p0 = ProducerTicketStream::producer(&n0);

	let mut c1 = n1
		.streams()
		.consumer::<Data1>()
		.with_stream_id("producer.ticket.stream")
		.build();

	let c2 = n2
		.streams()
		.consumer::<Data1>()
		.with_stream_id("producer.ticket.stream")
		.build();

	timeout_s(5, discover_all([&n0, &n1, &n2])).await??;

	timeout_s(3, c1.when().subscribed()).await?;

	sleep_s(2).await;
	assert!(
		!c2.when().subscribed().is_condition_met(),
		"no-ticket consumer should not be subscribed"
	);

	p0.send(Data1("producer ticket".into())).await?;
	let msg = timeout_s(3, c1.next()).await?.expect("expected message");
	assert_eq!(msg, Data1("producer ticket".into()));

	Ok(())
}

/// Verifies that a consumer-only `stream!` with `require_ticket` only
/// subscribes to producers with valid tickets.
#[tokio::test]
async fn stream_macro_consumer_require_ticket() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let (n0, n1, n2) = tokio::try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
	)?;

	let jwt_validator = JwtIssuer::default();

	n1.discovery()
		.add_ticket(jwt_validator.make_valid_ticket(&n1.local().id()));

	let mut c0 = ConsumerTicketStream::consumer(&n0);

	let mut p1 = n1
		.streams()
		.producer::<Data1>()
		.with_stream_id("consumer.ticket.stream")
		.build()?;

	let _p2 = n2
		.streams()
		.producer::<Data1>()
		.with_stream_id("consumer.ticket.stream")
		.build()?;

	timeout_s(5, discover_all([&n0, &n1, &n2])).await??;

	timeout_s(3, c0.when().subscribed()).await?;

	sleep_s(2).await;
	let producers = c0.producers().map(|p| *p.peer().id()).collect::<Vec<_>>();
	assert_eq!(producers.len(), 1, "should subscribe to 1 producer only");
	assert!(producers.contains(&n1.local().id()));

	p1.send(Data1("consumer ticket".into())).await?;
	let msg = timeout_s(3, c0.next()).await?.expect("expected message");
	assert_eq!(msg, Data1("consumer ticket".into()));

	Ok(())
}

/// Verifies that a `stream!` declaration with multiple `require_ticket`
/// entries gates both sides: only peers carrying valid tickets from **all**
/// issuers are connected.
#[tokio::test]
async fn stream_macro_multiple_require_ticket() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let issuer_a = JwtIssuer::new("issuer-alpha", "secret-alpha");
	let issuer_b = JwtIssuer::new("issuer-beta", "secret-beta");

	let (n0, n1, n2, n3) = tokio::try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
	)?;

	// n0 (producer) and n1 (consumer) carry tickets from BOTH issuers.
	n0.discovery()
		.add_ticket(issuer_a.make_valid_ticket(&n0.local().id()));
	n0.discovery()
		.add_ticket(issuer_b.make_valid_ticket(&n0.local().id()));
	n1.discovery()
		.add_ticket(issuer_a.make_valid_ticket(&n1.local().id()));
	n1.discovery()
		.add_ticket(issuer_b.make_valid_ticket(&n1.local().id()));

	// n2 only has issuer_a ticket → should be rejected.
	n2.discovery()
		.add_ticket(issuer_a.make_valid_ticket(&n2.local().id()));

	// n3 only has issuer_b ticket → should be rejected.
	n3.discovery()
		.add_ticket(issuer_b.make_valid_ticket(&n3.local().id()));

	let mut p0 = MultiTicketStream::producer(&n0);
	let mut c1 = MultiTicketStream::consumer(&n1);
	let c2 = MultiTicketStream::consumer(&n2);
	let c3 = MultiTicketStream::consumer(&n3);

	timeout_s(5, discover_all([&n0, &n1, &n2, &n3])).await??;

	// c1 should connect (both tickets valid on both sides).
	timeout_s(3, c1.when().subscribed()).await?;

	// c2 and c3 should not connect (missing one ticket each).
	sleep_s(3).await;
	assert!(
		!c2.when().subscribed().is_condition_met(),
		"consumer with only issuer_a ticket should not be subscribed"
	);
	assert!(
		!c3.when().subscribed().is_condition_met(),
		"consumer with only issuer_b ticket should not be subscribed"
	);

	// Data flows between n0 and n1.
	p0.send(Data1("multi-ticket-gated".into())).await?;
	let msg = timeout_s(3, c1.next()).await?.expect("expected message");
	assert_eq!(msg, Data1("multi-ticket-gated".into()));

	Ok(())
}
