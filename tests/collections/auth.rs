use {
	crate::utils::{
		DEFAULT_ISSUER,
		DEFAULT_SECRET,
		Hs256,
		Jwt,
		discover_all,
		jwt_builder,
		jwt_secret,
		jwt_validator,
		sleep_s,
		timeout_s,
		valid_expiry,
	},
	mosaik::{
		collections::{
			CollectionConfig,
			CollectionReader,
			CollectionWriter,
			StoreId,
			WriterOf,
		},
		*,
	},
};

/// Verifies that a Map collection gated with a JWT ticket validator only allows
/// peers with valid tickets to participate in the underlying group, and that
/// the writer can replicate data to the authorized reader.
#[tokio::test]
async fn map_auth_tickets() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let (n0, n1, n2) = tokio::try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
	)?;

	let builder = jwt_builder(DEFAULT_ISSUER, DEFAULT_SECRET);
	let validator = jwt_validator(DEFAULT_ISSUER, DEFAULT_SECRET);

	// n0 and n1 get valid tickets; n2 gets none
	n0.discovery()
		.add_ticket(builder.build(&n0.local().id(), valid_expiry()));
	n1.discovery()
		.add_ticket(builder.build(&n1.local().id(), valid_expiry()));

	timeout_s(5, discover_all([&n0, &n1, &n2])).await??;

	let config = CollectionConfig::default().require_ticket(validator.clone());

	let w = collections::Map::<u64, u64>::new_with_config(&n0, store_id, config);
	let r = collections::Map::<u64, u64>::reader_with_config(
		&n1,
		store_id,
		CollectionConfig::default().require_ticket(validator.clone()),
	);
	let r_unauth = collections::Map::<u64, u64>::reader_with_config(
		&n2,
		store_id,
		CollectionConfig::default().require_ticket(validator),
	);

	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.insert(1, 100)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(100));

	// n2 should never come online since it has no ticket
	sleep_s(3).await;
	assert!(
		r_unauth.is_empty(),
		"unauthorized reader should see no data"
	);

	Ok(())
}

/// Verifies Vec collection auth ticket gating.
#[tokio::test]
async fn vec_auth_tickets() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let (n0, n1, n2) = tokio::try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
	)?;

	let builder = jwt_builder(DEFAULT_ISSUER, DEFAULT_SECRET);
	let validator = jwt_validator(DEFAULT_ISSUER, DEFAULT_SECRET);

	n0.discovery()
		.add_ticket(builder.build(&n0.local().id(), valid_expiry()));
	n1.discovery()
		.add_ticket(builder.build(&n1.local().id(), valid_expiry()));

	timeout_s(5, discover_all([&n0, &n1, &n2])).await??;

	let config = || CollectionConfig::default().require_ticket(validator.clone());

	let w = collections::Vec::<u64>::new_with_config(&n0, store_id, config());
	let r = collections::Vec::<u64>::reader_with_config(&n1, store_id, config());
	let r_unauth =
		collections::Vec::<u64>::reader_with_config(&n2, store_id, config());

	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.push_back(42)).await??;
	timeout_s(5, r.when().reaches(ver)).await?;
	assert_eq!(r.get(0), Some(42));

	sleep_s(3).await;
	assert!(
		r_unauth.is_empty(),
		"unauthorized reader should see no data"
	);

	Ok(())
}

/// Verifies Set collection auth ticket gating.
#[tokio::test]
async fn set_auth_tickets() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let (n0, n1, n2) = tokio::try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
	)?;

	let builder = jwt_builder(DEFAULT_ISSUER, DEFAULT_SECRET);
	let validator = jwt_validator(DEFAULT_ISSUER, DEFAULT_SECRET);

	n0.discovery()
		.add_ticket(builder.build(&n0.local().id(), valid_expiry()));
	n1.discovery()
		.add_ticket(builder.build(&n1.local().id(), valid_expiry()));

	timeout_s(5, discover_all([&n0, &n1, &n2])).await??;

	let config = || CollectionConfig::default().require_ticket(validator.clone());

	let w = collections::Set::<u64>::new_with_config(&n0, store_id, config());
	let r = collections::Set::<u64>::reader_with_config(&n1, store_id, config());
	let r_unauth =
		collections::Set::<u64>::reader_with_config(&n2, store_id, config());

	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.insert(99)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert!(r.contains(&99));

	sleep_s(3).await;
	assert!(
		r_unauth.is_empty(),
		"unauthorized reader should see no data"
	);

	Ok(())
}

/// Verifies Cell collection auth ticket gating.
#[tokio::test]
async fn cell_auth_tickets() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let (n0, n1, n2) = tokio::try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
	)?;

	let builder = jwt_builder(DEFAULT_ISSUER, DEFAULT_SECRET);
	let validator = jwt_validator(DEFAULT_ISSUER, DEFAULT_SECRET);

	n0.discovery()
		.add_ticket(builder.build(&n0.local().id(), valid_expiry()));
	n1.discovery()
		.add_ticket(builder.build(&n1.local().id(), valid_expiry()));

	timeout_s(5, discover_all([&n0, &n1, &n2])).await??;

	let config = || CollectionConfig::default().require_ticket(validator.clone());

	let w = collections::Cell::<u64>::new_with_config(&n0, store_id, config());
	let r = collections::Cell::<u64>::reader_with_config(&n1, store_id, config());
	let r_unauth =
		collections::Cell::<u64>::reader_with_config(&n2, store_id, config());

	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.write(7)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.read(), Some(7));

	sleep_s(3).await;
	assert!(
		r_unauth.is_empty(),
		"unauthorized reader should see no data"
	);

	Ok(())
}

/// Verifies Once collection auth ticket gating.
#[tokio::test]
async fn once_auth_tickets() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let (n0, n1, n2) = tokio::try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
	)?;

	let builder = jwt_builder(DEFAULT_ISSUER, DEFAULT_SECRET);
	let validator = jwt_validator(DEFAULT_ISSUER, DEFAULT_SECRET);

	n0.discovery()
		.add_ticket(builder.build(&n0.local().id(), valid_expiry()));
	n1.discovery()
		.add_ticket(builder.build(&n1.local().id(), valid_expiry()));

	timeout_s(5, discover_all([&n0, &n1, &n2])).await??;

	let config = || CollectionConfig::default().require_ticket(validator.clone());

	let w = collections::Once::<u64>::new_with_config(&n0, store_id, config());
	let r = collections::Once::<u64>::reader_with_config(&n1, store_id, config());
	let r_unauth =
		collections::Once::<u64>::reader_with_config(&n2, store_id, config());

	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.write(42)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.read(), Some(42));

	sleep_s(3).await;
	assert!(
		r_unauth.is_empty(),
		"unauthorized reader should see no data"
	);

	Ok(())
}

/// Verifies `PriorityQueue` collection auth ticket gating.
#[tokio::test]
async fn priority_queue_auth_tickets() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let (n0, n1, n2) = tokio::try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
	)?;

	let builder = jwt_builder(DEFAULT_ISSUER, DEFAULT_SECRET);
	let validator = jwt_validator(DEFAULT_ISSUER, DEFAULT_SECRET);

	n0.discovery()
		.add_ticket(builder.build(&n0.local().id(), valid_expiry()));
	n1.discovery()
		.add_ticket(builder.build(&n1.local().id(), valid_expiry()));

	timeout_s(5, discover_all([&n0, &n1, &n2])).await??;

	let config = || CollectionConfig::default().require_ticket(validator.clone());

	let w = collections::PriorityQueue::<u64, u64, u64>::new_with_config(
		&n0,
		store_id,
		config(),
	);
	let r = collections::PriorityQueue::<u64, u64, u64>::reader_with_config(
		&n1,
		store_id,
		config(),
	);
	let r_unauth =
		collections::PriorityQueue::<u64, u64, u64>::reader_with_config(
			&n2,
			store_id,
			config(),
		);

	timeout_s(10, w.when().online()).await?;
	timeout_s(10, r.when().online()).await?;

	let ver = timeout_s(2, w.insert(10, 1, 100)).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get(&1), Some(100));

	sleep_s(3).await;
	assert!(
		r_unauth.is_empty(),
		"unauthorized reader should see no data"
	);

	Ok(())
}

/// Verifies that two collections with the same store id but different ticket
/// validators derive different group ids. This ensures that the validator
/// configuration is part of the group identity.
#[tokio::test]
async fn different_auths_produce_different_group_ids() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let store_id = StoreId::random();

	let n0 = Network::new(network_id).await?;

	let validator_a = jwt_validator("issuer-alpha", "secret-alpha");
	let validator_b = jwt_validator("issuer-beta", "secret-beta");

	let w_a = collections::Map::<u64, u64>::new_with_config(
		&n0,
		store_id,
		CollectionConfig::default().require_ticket(validator_a),
	);
	let w_b = collections::Map::<u64, u64>::new_with_config(
		&n0,
		store_id,
		CollectionConfig::default().require_ticket(validator_b),
	);
	let w_none = collections::Map::<u64, u64>::new(&n0, store_id);

	assert_ne!(
		w_a.group_id(),
		w_b.group_id(),
		"different validators should produce different group ids"
	);
	assert_ne!(
		w_a.group_id(),
		w_none.group_id(),
		"auth-gated and open collections should have different group ids"
	);
	assert_ne!(
		w_b.group_id(),
		w_none.group_id(),
		"auth-gated and open collections should have different group ids"
	);

	Ok(())
}

/// Verifies that the `collection!` macro supports `require_ticket` and that
/// the declared collection only allows authorized peers to participate.
#[tokio::test]
async fn collection_macro_with_require_ticket() -> anyhow::Result<()> {
	mosaik::collection!(
		AuthMap = mosaik::collections::Map<String, String>,
		"test.auth.macro.map",
		require_ticket: Jwt::with_key(
			Hs256::new(jwt_secret(DEFAULT_SECRET))
		).allow_issuer(DEFAULT_ISSUER),
	);

	let network_id = NetworkId::random();

	let (n0, n1, n2) = tokio::try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
	)?;

	let builder = jwt_builder(DEFAULT_ISSUER, DEFAULT_SECRET);

	// n0 and n1 get valid tickets; n2 gets none
	n0.discovery()
		.add_ticket(builder.build(&n0.local().id(), valid_expiry()));
	n1.discovery()
		.add_ticket(builder.build(&n1.local().id(), valid_expiry()));

	timeout_s(5, discover_all([&n0, &n1, &n2])).await??;

	let w: WriterOf<AuthMap> = timeout_s(10, AuthMap::online_writer(&n0)).await?;
	let r = timeout_s(10, AuthMap::online_reader(&n1)).await?;

	let ver = timeout_s(2, w.insert("key".into(), "value".into())).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get("key"), Some("value".into()));

	// n2 has no ticket — its reader should never receive data
	let r_unauth = AuthMap::reader(&n2);
	sleep_s(3).await;
	assert!(
		r_unauth.is_empty(),
		"unauthorized macro reader should see no data"
	);

	Ok(())
}

/// Verifies that the `collection!` macro supports multiple `require_ticket`
/// entries. Peers must satisfy ALL validators to participate.
#[tokio::test]
async fn collection_macro_with_multiple_tickets() -> anyhow::Result<()> {
	mosaik::collection!(
		MultiAuthMap = mosaik::collections::Map<String, String>,
		"test.multi.auth.macro.map",
		require_ticket: Jwt::with_key(
			Hs256::new(jwt_secret("secret-alpha"))
		).allow_issuer("issuer-alpha"),
		require_ticket: Jwt::with_key(
			Hs256::new(jwt_secret("secret-beta"))
		).allow_issuer("issuer-beta"),
	);

	let network_id = NetworkId::random();

	let (n0, n1, n2) = tokio::try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
	)?;

	let builder_a = jwt_builder("issuer-alpha", "secret-alpha");
	let builder_b = jwt_builder("issuer-beta", "secret-beta");

	// n0 and n1 carry tickets from both issuers
	n0.discovery()
		.add_ticket(builder_a.build(&n0.local().id(), valid_expiry()));
	n0.discovery()
		.add_ticket(builder_b.build(&n0.local().id(), valid_expiry()));
	n1.discovery()
		.add_ticket(builder_a.build(&n1.local().id(), valid_expiry()));
	n1.discovery()
		.add_ticket(builder_b.build(&n1.local().id(), valid_expiry()));

	// n2 only carries a ticket from issuer-alpha (missing issuer-beta)
	n2.discovery()
		.add_ticket(builder_a.build(&n2.local().id(), valid_expiry()));

	timeout_s(5, discover_all([&n0, &n1, &n2])).await??;

	let w: WriterOf<MultiAuthMap> =
		timeout_s(10, MultiAuthMap::online_writer(&n0)).await?;
	let r = timeout_s(10, MultiAuthMap::online_reader(&n1)).await?;

	let ver = timeout_s(2, w.insert("hello".into(), "world".into())).await??;
	timeout_s(2, r.when().reaches(ver)).await?;
	assert_eq!(r.get("hello"), Some("world".into()));

	// n2 only has one of the two required tickets — should be excluded
	let r_partial = MultiAuthMap::reader(&n2);
	sleep_s(3).await;
	assert!(
		r_partial.is_empty(),
		"reader with only one of two required tickets should see no data"
	);

	Ok(())
}

/// Verifies that collections declared with the `collection!` macro and
/// different ticket validators derive different group ids, ensuring that the
/// validator configuration is part of the group identity.
#[tokio::test]
async fn macros_different_validators_group_ids() -> anyhow::Result<()> {
	mosaik::collection!(
		AuthMapA = mosaik::collections::Map<String, String>,
		"test.auth.macro.map.a",
		require_ticket: Jwt::with_key(
			Hs256::new(jwt_secret("secret-a"))
		).allow_issuer("issuer-a"),
	);

	mosaik::collection!(
		AuthMapB = mosaik::collections::Map<String, String>,
		"test.auth.macro.map.b",
		require_ticket: Jwt::with_key(
			Hs256::new(jwt_secret("secret-b"))
		).allow_issuer("issuer-b"),
	);

	let network_id = NetworkId::random();
	let n = Network::new(network_id).await?;

	let w_a: WriterOf<AuthMapA> =
		timeout_s(10, AuthMapA::online_writer(&n)).await?;
	let w_b: WriterOf<AuthMapB> =
		timeout_s(10, AuthMapB::online_writer(&n)).await?;

	assert_ne!(
		w_a.group_id(),
		w_b.group_id(),
		"collections with different validators should have different group ids"
	);

	Ok(())
}
