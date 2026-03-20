use {
	crate::utils::{discover_all, timeout_ms, timeout_s},
	mosaik::{
		collections::{
			CollectionDef,
			CollectionReader,
			CollectionWriter,
			ReaderDef,
			StoreId,
		},
		*,
	},
};

// Basic smoke: writer + reader, write-once semantics
#[tokio::test]
async fn smoke() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Once::<u64>::new(&n0, store_id);
	let r = mosaik::collections::Once::<u64>::reader(&n1, store_id);

	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// -- empty state --
	assert!(r.is_empty());
	assert!(r.is_none());
	assert!(!r.is_some());
	assert_eq!(r.read(), None);
	assert_eq!(r.get(), None);

	// -- first write succeeds --
	let ver = timeout_s(2, w.write(42)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert!(!r.is_empty());
	assert!(r.is_some());
	assert_eq!(r.read(), Some(42));
	assert_eq!(r.get(), Some(42));

	// -- second write is ignored --
	let ver = timeout_s(2, w.write(99)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.read(), Some(42)); // still the first value

	// -- third write is also ignored --
	let ver = timeout_s(2, w.set(123)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.read(), Some(42)); // still the first value

	Ok(())
}

// Late-joining reader catches up via snapshot
#[tokio::test]
async fn catchup_single_reader() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let w = mosaik::collections::Once::<u64>::writer(&n0, store_id);
	timeout_s(10, w.when().online()).await?;

	// write the one-and-only value, then issue more writes (all ignored)
	timeout_s(2, w.write(999)).await??;
	for i in 0u64..50 {
		timeout_s(2, w.write(i)).await??;
	}
	let final_ver = timeout_s(2, w.write(0)).await??;
	timeout_s(2, w.when().reaches(final_ver)).await?;

	// late-joining reader
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let r = mosaik::collections::Once::<u64>::reader(&n1, store_id);
	timeout_s(10, r.when().online()).await?;
	timeout_s(10, r.when().reaches(final_ver)).await?;

	// only the first value was kept
	assert_eq!(r.read(), Some(999));

	Ok(())
}

// Multiple readers all converge on the same value
#[tokio::test]
async fn multiple_readers() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n_w = Network::new(network_id).await?;
	let n_r1 = Network::new(network_id).await?;
	let n_r2 = Network::new(network_id).await?;
	let n_r3 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n_w, &n_r1, &n_r2, &n_r3])).await??;

	let w = mosaik::collections::Once::<u64>::writer(&n_w, store_id);
	let r1 = mosaik::collections::Once::<u64>::reader(&n_r1, store_id);
	let r2 = mosaik::collections::Once::<u64>::reader(&n_r2, store_id);
	let r3 = mosaik::collections::Once::<u64>::reader(&n_r3, store_id);

	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r1.when().online()).await?;
	timeout_s(10, r2.when().online()).await?;
	timeout_s(10, r3.when().online()).await?;

	let ver = timeout_s(2, w.write(42)).await??;
	timeout_s(2, r1.when().reaches(ver)).await?;
	timeout_s(2, r2.when().reaches(ver)).await?;
	timeout_s(2, r3.when().reaches(ver)).await?;

	for r in [&r1, &r2, &r3] {
		assert_eq!(r.read(), Some(42));
	}

	// subsequent writes do not change value
	let ver = timeout_s(2, w.write(100)).await??;
	timeout_s(2, r1.when().reaches(ver)).await?;
	timeout_s(2, r2.when().reaches(ver)).await?;
	timeout_s(2, r3.when().reaches(ver)).await?;

	for r in [&r1, &r2, &r3] {
		assert_eq!(r.read(), Some(42));
	}

	Ok(())
}

// Multiple writers — first write across all writers wins
#[tokio::test]
async fn multiple_writers() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	let n2 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1, &n2])).await??;

	let w0 = mosaik::collections::Once::<u64>::writer(&n0, store_id);
	let w1 = mosaik::collections::Once::<u64>::writer(&n1, store_id);
	let r = mosaik::collections::Once::<u64>::reader(&n2, store_id);

	timeout_s(10, w0.when().online()).await?;
	timeout_s(10, w1.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	// writer 0 sets the value first
	let v0 = timeout_s(2, w0.write(10)).await??;
	timeout_s(2, r.when().reaches(v0)).await?;
	assert_eq!(r.read(), Some(10));

	// writer 1 tries to overwrite — ignored
	let v1 = timeout_s(2, w1.write(20)).await??;
	timeout_s(2, r.when().reaches(v1)).await?;
	assert_eq!(r.read(), Some(10)); // still the first value

	// writer 0 tries again — also ignored
	let v2 = timeout_s(2, w0.write(30)).await??;
	timeout_s(2, r.when().reaches(v2)).await?;
	timeout_s(2, w0.when().reaches(v2)).await?;
	timeout_s(2, w1.when().reaches(v2)).await?;

	// all nodes agree on the first value
	assert_eq!(w0.read(), Some(10));
	assert_eq!(w1.read(), Some(10));
	assert_eq!(r.read(), Some(10));

	Ok(())
}

// Writer reads its own write
#[tokio::test]
async fn writer_reads_own_write() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let w = mosaik::collections::Once::<String>::writer(&n0, store_id);
	timeout_s(10, w.when().online()).await?;

	let ver = timeout_s(2, w.write("hello".into())).await??;
	timeout_s(2, w.when().reaches(ver)).await?;

	assert_eq!(w.read(), Some("hello".into()));
	assert!(!w.is_empty());

	// second write ignored
	let ver = timeout_s(2, w.write("world".into())).await??;
	timeout_s(2, w.when().reaches(ver)).await?;

	assert_eq!(w.read(), Some("hello".into()));

	Ok(())
}

// await_value blocks until a value is set, then returns it
#[tokio::test]
async fn await_value() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = mosaik::collections::Once::<u64>::writer(&n0, store_id);
	let r = mosaik::collections::Once::<u64>::reader(&n1, store_id);

	// reader awaits a value that hasn't been written yet
	let handle = tokio::spawn(async move { r.await_value().await });

	// give the reader time to start waiting, then write
	timeout_s(10, w.when().online()).await?;
	let ver = timeout_s(2, w.write(42)).await??;

	// await_value should resolve with the written value
	let got = timeout_s(10, handle).await??;
	assert_eq!(got, 42);

	// calling await_value when a value is already present returns
	// immediately
	timeout_s(2, w.when().reaches(ver)).await?;
	let got = timeout_ms(100, w.await_value()).await?;
	assert_eq!(got, 42);

	Ok(())
}

// Late writer catches up and sees existing value
#[tokio::test]
async fn late_writer_catchup() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;
	let w0 = mosaik::collections::Once::<u64>::writer(&n0, store_id);
	timeout_s(10, w0.when().online()).await?;

	let ver = timeout_s(2, w0.write(42)).await??;
	timeout_s(2, w0.when().reaches(ver)).await?;

	// late-joining writer
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w1 = mosaik::collections::Once::<u64>::writer(&n1, store_id);
	timeout_s(10, w1.when().online()).await?;
	timeout_s(10, w1.when().reaches(ver)).await?;

	// late writer sees existing value
	assert_eq!(w1.read(), Some(42));

	// late writer's write is ignored
	let v1 = timeout_s(2, w1.write(99)).await??;
	timeout_s(2, w0.when().reaches(v1)).await?;

	assert_eq!(w0.read(), Some(42));
	assert_eq!(w1.read(), Some(42));

	Ok(())
}

#[tokio::test]
async fn construct_from_def() -> anyhow::Result<()> {
	const DEF: CollectionDef<mosaik::collections::Once<String>> =
		CollectionDef::new(unique_id!("test1"));

	const READER_DEF: ReaderDef<mosaik::collections::Once<String>> =
		DEF.as_reader();

	let network_id = NetworkId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w = DEF.writer(&n0);
	let r = DEF.reader(&n1);

	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.set("hello".into())).await??;
	timeout_s(2, w.when().reaches(ver)).await?;

	assert!(w.is_some());

	timeout_s(2, r.when().reaches(ver)).await?;
	assert!(r.is_some());

	let val = r.get();
	assert_eq!(val, Some("hello".into()));

	let n2 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1, &n2])).await??;

	let r2 = READER_DEF.open(&n2);
	timeout_s(10, r2.when().online()).await?;
	timeout_s(10, r2.when().reaches(ver)).await?;
	assert!(r2.is_some());
	assert_eq!(r2.get(), Some("hello".into()));

	Ok(())
}

#[tokio::test]
async fn construct_from_macro() -> anyhow::Result<()> {
	mosaik::collection!(
		TestOnce = mosaik::collections::Once<String>,
		"test.once.macro"
	);

	let network_id = NetworkId::random();

	let n0 = Network::new(network_id).await?;
	let n1 = Network::new(network_id).await?;
	timeout_s(10, discover_all([&n0, &n1])).await??;

	let w: WriterOf<TestOnce> = TestOnce::writer(&n0);
	let r: ReaderOf<TestOnce> = TestOnce::reader(&n1);

	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.set("hello".into())).await??;
	timeout_s(2, r.when().reaches(ver)).await?;

	assert!(r.is_some());
	assert_eq!(r.get(), Some("hello".into()));

	Ok(())
}
