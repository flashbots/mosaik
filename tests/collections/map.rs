use {
	crate::utils::discover_all,
	mosaik::{collections::StoreId, *},
};

#[tokio::test]
async fn smoke() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	discover_all([&n0, &n1]).await?;

	let s0 = mosaik::collections::Map::<String, u64>::writer(&n0, store_id);
	let s1 = mosaik::collections::Map::<String, u64>::reader(&n1, store_id);

	s0.when().online().await;
	s1.when().online().await;

	s0.insert(s("foo"), 42).await?;

	s1.when().updated().await;
	let value = s1.get(&s("foo"));
	assert_eq!(value, Some(42));
	assert!(s1.contains_key(&s("foo")));

	let ver = s0
		.extend(vec![(s("bar"), 7), (s("baz"), 13), (s("qux"), 21)])
		.await?;

	s1.when().reaches(ver).await;

	assert_eq!(s1.get(&s("bar")), Some(7));
	assert!(s1.contains_key(&s("bar")));

	assert_eq!(s1.get(&s("baz")), Some(13));
	assert!(s1.contains_key(&s("baz")));

	assert_eq!(s1.get(&s("qux")), Some(21));
	assert!(s1.contains_key(&s("qux")));

	let ver = s0.remove(s("foo")).await?;
	s1.when().reaches(ver).await;
	assert_eq!(s1.get(&s("foo")), None);
	assert!(!s1.contains_key(&s("foo")));

	Ok(())
}

fn s(s: &str) -> String {
	s.to_string()
}
