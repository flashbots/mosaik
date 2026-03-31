use {
	super::*,
	crate::utils::{discover_all, sleep_s, timeout_s},
	futures::{SinkExt, StreamExt},
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
	pub(crate) producer ProducerOnlyStream = Data1, "producer.only",
		require: |_peer| true,
);

// Consumer only with require config.
declare::stream!(
	pub(crate) consumer ConsumerOnlyStream = Data1, "consumer.only",
		require: |_peer| true,
);

// Full with side-prefixed online_when.
declare::stream!(
	pub(crate) ConfiguredStream = Data1, "configured.stream",
		require: |_peer| true,
		producer online_when: |c| c.minimum_of(1),
);

// Producer-only stream that requires consumers to carry the "trusted" tag.
declare::stream!(
	pub producer TaggedProducerStream = Data2, "tagged.producer.stream",
		require: |peer| peer.tags().contains(&"trusted".into()),
);

// Consumer-only stream that only subscribes to producers with the "trusted" tag.
declare::stream!(
	pub consumer TaggedConsumerStream = Data2, "tagged.consumer.stream",
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
/// predicate rejects consumers that do not satisfy it and accepts those that do.
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
	assert_eq!(producers.len(), 1, "should be subscribed to exactly 1 producer");
	assert!(
		producers.contains(&n1.local().id()),
		"should be subscribed to the trusted producer only"
	);

	// Verify data flows from the one accepted producer.
	timeout_s(3, p1.send(Data2("hello from trusted producer".into()))).await??;
	let msg = timeout_s(3, c0.next())
		.await?
		.expect("expected message");
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
