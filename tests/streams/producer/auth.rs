use {
	super::*,
	futures::{SinkExt, StreamExt},
	mosaik::*,
};

#[tokio::test]
async fn by_tag() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let n0 = Network::new(network_id).await?;

	let mut p0 = n0
		.streams()
		.producer::<Data1>()
		.accept_if(|peer| peer.tags().contains(&"tag1".into()))
		.build()?;

	let n1 = Network::builder(network_id)
		.with_discovery(
			discovery::Config::builder()
				.with_tags(["tag1", "tag2"])
				.with_bootstrap(n0.local().id()),
		)
		.build()
		.await?;

	let n2 = Network::builder(network_id)
		.with_discovery(
			discovery::Config::builder()
				.with_tags(["tag3", "tag4"])
				.with_bootstrap(n0.local().id()),
		)
		.build()
		.await?;

	let mut c1 = n1.streams().consume::<Data1>();
	let _c2 = n2.streams().consume::<Data1>();

	c1.when().subscribed().await;

	p0.send(Data1("hello from n0".into())).await?;
	let msg1 = c1.next().await.expect("message from c1");
	assert_eq!(msg1.0, "hello from n0");

	Ok(())
}
