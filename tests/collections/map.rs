use {
	crate::utils::{discover_all, timeout_s},
	mosaik::{collections::StoreId, *},
};

#[tokio::test]
async fn smoke_no_catchup() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let s0 = mosaik::collections::Map::<String, u64>::writer(&n0, store_id);
	let s1 = mosaik::collections::Map::<String, u64>::reader(&n1, store_id);

	timeout_s(10, s0.when().online()).await?;
	timeout_s(10, s1.when().online()).await?;

	s0.insert(s("foo"), 42).await?;

	timeout_s(2, s1.when().updated()).await?;
	let value = s1.get(&s("foo"));
	assert_eq!(value, Some(42));
	assert!(s1.contains_key(&s("foo")));

	let ver = timeout_s(
		2,
		s0.extend(vec![(s("bar"), 7), (s("baz"), 13), (s("qux"), 21)]),
	)
	.await??;

	timeout_s(2, s1.when().reaches(ver)).await?;

	assert_eq!(s1.get(&s("bar")), Some(7));
	assert!(s1.contains_key(&s("bar")));

	assert_eq!(s1.get(&s("baz")), Some(13));
	assert!(s1.contains_key(&s("baz")));

	assert_eq!(s1.get(&s("qux")), Some(21));
	assert!(s1.contains_key(&s("qux")));

	assert!(s1.contains_key(&s("foo")));
	let ver = timeout_s(2, s0.remove(s("foo"))).await??;
	timeout_s(2, s1.when().reaches(ver)).await?;
	assert_eq!(s1.get(&s("foo")), None);
	assert!(!s1.contains_key(&s("foo")));

	Ok(())
}

#[tokio::test]
async fn smoke_catchup() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;

	let s0 = mosaik::collections::Map::<String, u64>::writer(&n0, store_id);
	timeout_s(10, s0.when().online()).await?;

	s0.insert(s("foo"), 42).await?;

	for i in 0..100 {
		timeout_s(
			2,
			s0.extend(vec![
				(format!("key{i}_1"), i + 7),
				(format!("key{i}_2"), i + 13),
				(format!("key{i}_3"), i + 21),
			]),
		)
		.await??;
	}

	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let s1 = mosaik::collections::Map::<String, u64>::reader(&n1, store_id);
	timeout_s(10, s1.when().online()).await?;

	assert_eq!(s1.len(), 1 + 100 * 3);
	assert_eq!(s1.get(&s("foo")), Some(42));

	for i in 0..100 {
		assert_eq!(s1.get(&format!("key{i}_1")), Some(i + 7));
		assert_eq!(s1.get(&format!("key{i}_2")), Some(i + 13));
		assert_eq!(s1.get(&format!("key{i}_3")), Some(i + 21));
	}

	let pos = timeout_s(2, s0.insert(s("final_key_1"), 123)).await??;
	timeout_s(2, s1.when().reaches(pos)).await?;

	assert_eq!(s0.get(&s("final_key_1")), Some(123));
	assert_eq!(s1.get(&s("final_key_1")), Some(123));

	Ok(())
}

fn s(s: &str) -> String {
	s.to_string()
}
