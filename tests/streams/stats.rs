use {
	super::*,
	crate::utils::*,
	futures::{SinkExt, StreamExt},
	mosaik::*,
};

#[tokio::test]
async fn smoke() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	let n2 = Network::new(network_id).await?;

	let mut p0 = n0.streams().produce::<Data1>();

	// start with only one consumer, then add another later
	// and see if the stats update correctly.
	let mut c1 = n1.streams().consume::<Data1>();

	discover_all([&n0, &n1, &n2]).await?;

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

	assert_eq!(p0_subs.len(), 2);
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
