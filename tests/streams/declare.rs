use {
	super::*,
	crate::utils::{discover_all, timeout_s},
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

// Producer only with accept_if config.
declare::stream!(pub(crate) producer ProducerOnlyStream = Data1, "producer.only",
	accept_if: |_peer| true,
);

// Consumer only with subscribe_if config.
declare::stream!(pub(crate) consumer ConsumerOnlyStream = Data1, "consumer.only",
	subscribe_if: |_peer| true,
);

// Full with side-prefixed online_when.
declare::stream!(pub(crate) ConfiguredStream = Data1, "configured.stream",
	accept_if: |_peer| true,
	producer online_when: |c| c.minimum_of(1),
	subscribe_if: |_peer| true,
);

// Producer that only accepts peers with a specific tag.
declare::stream!(pub TaggedStream = Data2, "tagged.stream",
	accept_if: |peer| peer.tags().contains(&"trusted".into()),
);

declare::stream!(
	/// A well-known price feed.
	pub(crate) PriceFeed = Data2, "oracle.price"
);

// With a constant stream id expression.
const CONST_STREAM_ID: StreamId = unique_id!("const.stream");
declare::stream!(pub(crate) ConstIdStream = Data1, CONST_STREAM_ID);

// Constant stream id with config entries.
declare::stream!(pub(crate) ConstIdConfiguredStream = Data1, CONST_STREAM_ID,
	accept_if: |_peer| true,
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

#[tokio::test]
async fn stream_macro_tagged_accept() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let n0 = Network::new(network_id).await?;

	// n1 announces with the "trusted" tag.
	let n1 = Network::builder(network_id)
		.with_discovery(discovery::Config::builder().with_tags("trusted"))
		.build()
		.await?;

	// n2 has no tags — the accept_if predicate should reject it.
	let n2 = Network::new(network_id).await?;

	let mut p0 = TaggedStream::producer(&n0);

	let mut c1 = n1
		.streams()
		.consumer::<Data2>()
		.with_stream_id("tagged.stream")
		.build();

	let c2 = n2
		.streams()
		.consumer::<Data2>()
		.with_stream_id("tagged.stream")
		.build();

	discover_all([&n0, &n1, &n2]).await?;

	// Trusted consumer should connect.
	timeout_s(3, c1.when().subscribed()).await?;

	timeout_s(3, p0.send(Data2("hello tagged".into()))).await??;

	let msg = timeout_s(3, c1.next())
		.await?
		.expect("expected message from trusted consumer");
	assert_eq!(msg, Data2("hello tagged".into()));

	// Untrusted consumer should not receive anything (not subscribed).
	assert!(
		!c2.when().subscribed().is_condition_met(),
		"untrusted consumer should not be subscribed"
	);

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
