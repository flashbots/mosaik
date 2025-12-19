use {
	super::*,
	crate::utils::*,
	futures::{SinkExt, StreamExt},
	mosaik::*,
};

/// Validates that producers correctly track statistics about their consumers.
#[tokio::test]
async fn producer() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	let n2 = Network::new(network_id).await?;

	let mut p0 = n0.streams().produce::<Data1>();
	assert_eq!(p0.consumers().count(), 0);

	// start with only one consumer, then add another later
	// and see if the stats update correctly.
	let mut c1 = n1.streams().consume::<Data1>();

	timeout_s(1, discover_all([&n0, &n1, &n2])).await??;

	// ensure consumer is subscribed
	timeout_s(3, c1.when().subscribed()).await?;

	// check initial stats
	let p0_subs: Vec<_> = p0.consumers().collect();
	assert_eq!(p0_subs.len(), 1);
	assert_eq!(p0_subs[0].stats().datums(), 0);
	assert_eq!(p0_subs[0].stats().bytes(), 0);

	let c1_subs: Vec<_> = c1.producers().collect();
	assert_eq!(c1_subs.len(), 1);
	assert_eq!(c1_subs[0].stats().datums(), 0);
	assert_eq!(c1_subs[0].stats().bytes(), 0);

	// send one datum and check stats update
	p0.send(Data1("test".into())).await?;
	assert_eq!(c1.next().await, Some(Data1("test".into())));

	let p0_subs: Vec<_> = p0.consumers().collect();
	assert_eq!(p0_subs.len(), 1);
	assert_eq!(p0_subs[0].stats().datums(), 1);
	assert_eq!(p0_subs[0].stats().bytes(), 5); // serialized "test" is 5 bytes

	let c1_subs: Vec<_> = c1.producers().collect();
	assert_eq!(c1_subs.len(), 1);
	assert_eq!(c1_subs[0].stats().datums(), 1);
	assert_eq!(c1_subs[0].stats().bytes(), 5); // serialized "test" is 5 bytes

	// add another consumer
	let mut c2 = n2.streams().consume::<Data1>();
	timeout_s(3, c2.when().subscribed()).await?;

	// send another datum and check stats update for both consumers
	p0.send(Data1("data2".into())).await?;
	assert_eq!(c1.next().await, Some(Data1("data2".into())));
	assert_eq!(c2.next().await, Some(Data1("data2".into())));

	let p0_subs: Vec<_> = p0.consumers().collect();

	let sub_c1 = p0_subs
		.iter()
		.find(|s| *s.peer().id() == n1.local().id())
		.unwrap();
	let sub_c2 = p0_subs
		.iter()
		.find(|s| *s.peer().id() == n2.local().id())
		.unwrap();

	assert_eq!(sub_c1.stats().datums(), 2);
	assert_eq!(sub_c1.stats().bytes(), 11); // serialized "data2" is 6 bytes
	assert_eq!(sub_c2.stats().datums(), 1);
	assert_eq!(sub_c2.stats().bytes(), 6); // serialized "data2" is 6 bytes

	let c1_subs: Vec<_> = c1.producers().collect();
	assert_eq!(c1_subs.len(), 1);
	assert_eq!(c1_subs[0].stats().datums(), 2);
	assert_eq!(c1_subs[0].stats().bytes(), 11); // serialized "data2" is 6 bytes

	let c2_subs: Vec<_> = c2.producers().collect();
	assert_eq!(c2_subs.len(), 1);
	assert_eq!(c2_subs[0].stats().datums(), 1);
	assert_eq!(c2_subs[0].stats().bytes(), 6); // serialized "data2" is 6 bytes

	Ok(())
}

/// Validates that consumers correctly track statistics about their producers.
#[tokio::test]
async fn consumer() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	let n2 = Network::new(network_id).await?;

	timeout_s(2, discover_all([&n0, &n1, &n2])).await??;

	let mut c0 = n0.streams().consume::<Data1>();
	assert_eq!(c0.producers().count(), 0);

	let mut p1 = n1.streams().produce::<Data1>();
	timeout_s(2, c0.when().subscribed().minimum_of(1)).await?;

	let producers = c0.producers().collect::<Vec<_>>();
	assert_eq!(producers.len(), 1);
	assert_eq!(producers[0].stats().datums(), 0);
	assert_eq!(producers[0].stats().bytes(), 0);

	timeout_s(2, p1.send(Data1("hello".into()))).await??;
	let received = timeout_s(1, c0.next()).await?.unwrap();
	assert_eq!(received, Data1("hello".into()));

	let producers = c0.producers().collect::<Vec<_>>();
	assert_eq!(producers.len(), 1);
	assert_eq!(producers[0].stats().datums(), 1);
	assert_eq!(producers[0].stats().bytes(), 6); // serialized "hello" is 6 bytes

	let mut p2 = n2.streams().produce::<Data1>();
	timeout_s(2, c0.when().subscribed().minimum_of(2)).await?;

	timeout_s(2, p2.send(Data1("world".into()))).await??;
	let received = timeout_s(1, c0.next()).await?.unwrap();
	assert_eq!(received, Data1("world".into()));

	let producers = c0.producers().collect::<Vec<_>>();
	assert_eq!(producers.len(), 2);

	let prod1 = producers
		.iter()
		.find(|p| *p.peer().id() == n1.local().id())
		.unwrap();

	let prod2 = producers
		.iter()
		.find(|p| *p.peer().id() == n2.local().id())
		.unwrap();

	assert_eq!(prod1.stats().datums(), 1);
	assert_eq!(prod1.stats().bytes(), 6); // serialized "world" is 6 bytes

	assert_eq!(prod2.stats().datums(), 1);
	assert_eq!(prod2.stats().bytes(), 6); // serialized "world" is 6 bytes

	timeout_s(2, p1.send(Data1("again".into()))).await??;
	let received = timeout_s(2, c0.next()).await?.unwrap();
	assert_eq!(received, Data1("again".into()));

	let producers = c0.producers().collect::<Vec<_>>();
	assert_eq!(producers.len(), 2);

	let prod1 = producers
		.iter()
		.find(|p| *p.peer().id() == n1.local().id())
		.unwrap();

	let prod2 = producers
		.iter()
		.find(|p| *p.peer().id() == n2.local().id())
		.unwrap();

	assert_eq!(prod1.stats().datums(), 2);
	assert_eq!(prod1.stats().bytes(), 12); // serialized "again" is 6 bytes

	assert_eq!(prod2.stats().datums(), 1);
	assert_eq!(prod2.stats().bytes(), 6); // serialized "world" is 6 bytes

	assert_eq!(c0.stats().datums(), 3); // total datums from all producers
	assert_eq!(c0.stats().bytes(), 18); // total bytes from all producers

	Ok(())
}
