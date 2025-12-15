use {
	super::*,
	mosaik::{streams::producer::BuilderError, *},
};

#[tokio::test]
async fn multiple_producers_one_stream() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let n0 = Network::new(network_id).await?;

	tracing::debug!("n0: {}", n0.local().id());

	let p0 = n0
		.streams()
		.producer::<Data1>()
		.accept_if(|peer| peer.tags().contains(&"tag1".into()))
		.build()
		.unwrap();

	let p0_dup = n0
		.streams()
		.producer::<Data1>()
		.accept_if(|_| false)
		.build();

	// should error because a producer for Data1 already exists
	// and we are using the builder API to create a new one with custom
	// configuration
	assert!(matches!(p0_dup, Err(BuilderError::AlreadyExists(_))));
	let Err(BuilderError::AlreadyExists(p0_dup_err)) = p0_dup else {
		panic!("expected AlreadyExists error");
	};

	assert_eq!(p0.stream_id(), p0_dup_err.stream_id());

	// create another producer for the same stream using the default API
	let p0_dup = n0.streams().produce::<Data1>();
	assert_eq!(p0.stream_id(), p0_dup.stream_id());

	Ok(())
}
