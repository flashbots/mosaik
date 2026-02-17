use mosaik::{collections::StoreId, *};

#[tokio::test]
async fn vec_smoke() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let s0 = mosaik::collections::Vec::<u64>::writer(&n0, store_id);

	let n1 = Network::new(network_id).await?;
	let s1 = mosaik::collections::Vec::<u64>::reader(&n1, store_id);

	s0.when().online().await;
	s1.when().online().await;

	s0.push(42).await?;

	s1.when().updated().await;
	let value = s1.get(0);
	assert_eq!(value, Some(42));

	let ver = s0.extend(vec![7, 13, 21]).await?;

	s1.when().reaches(ver).await;

	assert_eq!(s1.get(1), Some(7));
	assert_eq!(s1.get(2), Some(13));
	assert_eq!(s1.get(3), Some(21));

	let ver = s0.remove(1).await?;
	s1.when().reaches(ver).await;
	assert_eq!(s1.get(1), None);

	Ok(())
}
