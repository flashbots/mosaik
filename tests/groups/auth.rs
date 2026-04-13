use {
	super::*,
	crate::utils::{
		DEFAULT_ISSUER,
		DEFAULT_SECRET,
		discover_all,
		expired_expiry,
		jwt_builder,
		jwt_validator,
		sleep_s,
		timeout_s,
		valid_expiry,
	},
	mosaik::{GroupKey, Network, NetworkId, tickets::TicketValidator},
};

/// Verifies that peers with valid JWT tickets can bond within an auth-gated
/// group, while peers without tickets or with expired tickets are excluded.
#[tokio::test]
async fn jwt_authorized_peers_can_bond() -> anyhow::Result<()> {
	const T: u64 = 8;

	let network_id = NetworkId::random();
	let key = GroupKey::from_secret("auth-group-secret");

	// n0 and n1 will have valid tickets; n2 has an expired ticket; n3 has none
	let (n0, n1, n2, n3) = tokio::try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
	)?;

	let builder = jwt_builder(DEFAULT_ISSUER, DEFAULT_SECRET);
	let jwt_validator = jwt_validator(DEFAULT_ISSUER, DEFAULT_SECRET);

	n0.discovery()
		.add_ticket(builder.build(&n0.local().id(), valid_expiry()));
	n1.discovery()
		.add_ticket(builder.build(&n1.local().id(), valid_expiry()));
	n2.discovery()
		.add_ticket(builder.build(&n2.local().id(), expired_expiry()));
	// n3 adds no ticket

	timeout_s(5, discover_all([&n0, &n1, &n2, &n3])).await??;

	let g0 = n0
		.groups()
		.with_key(key)
		.with_state_machine(Counter::default())
		.require_ticket(jwt_validator.clone())
		.join();

	let g1 = n1
		.groups()
		.with_key(key)
		.with_state_machine(Counter::default())
		.require_ticket(jwt_validator.clone())
		.join();

	let g2 = n2
		.groups()
		.with_key(key)
		.with_state_machine(Counter::default())
		.require_ticket(jwt_validator.clone())
		.join();

	let g3 = n3
		.groups()
		.with_key(key)
		.with_state_machine(Counter::default())
		.require_ticket(jwt_validator.clone())
		.join();

	// n0 and n1 should bond with each other
	timeout_s(T, ensure_bonds_formed(&g0, &n0, &[&n1], "g0")).await?;
	timeout_s(T, ensure_bonds_formed(&g1, &n1, &[&n0], "g1")).await?;

	// Allow time for n2 and n3 to attempt bonding (they should be rejected)
	sleep_s(3).await;

	// n0 and n1 should only be bonded with each other
	let g0_bonds: Vec<_> = g0.bonds().iter().map(|b| *b.peer().id()).collect();
	assert_eq!(g0_bonds.len(), 1, "g0 should have exactly one bond");
	assert!(g0_bonds.contains(&n1.local().id()));

	let g1_bonds: Vec<_> = g1.bonds().iter().map(|b| *b.peer().id()).collect();
	assert_eq!(g1_bonds.len(), 1, "g1 should have exactly one bond");
	assert!(g1_bonds.contains(&n0.local().id()));

	// n2 (expired ticket) and n3 (no ticket) should have no bonds
	assert_eq!(
		g2.bonds().len(),
		0,
		"g2 should have no bonds (expired ticket)"
	);
	assert_eq!(g3.bonds().len(), 0, "g3 should have no bonds (no ticket)");

	Ok(())
}

/// Verifies that an established bond is terminated when a peer's ticket is
/// revoked (i.e. removed from its discovery entry), as detected via the
/// `PeerEntryUpdate` message path in the bond worker.
#[tokio::test]
async fn bond_terminated_on_jwt_ticket_revocation() -> anyhow::Result<()> {
	const T: u64 = 8;

	let network_id = NetworkId::random();
	let key = GroupKey::from_secret("auth-revocation-test-secret");

	let (n0, n1) =
		tokio::try_join!(Network::new(network_id), Network::new(network_id))?;

	let builder = jwt_builder(DEFAULT_ISSUER, DEFAULT_SECRET);
	let jwt_validator = jwt_validator(DEFAULT_ISSUER, DEFAULT_SECRET);

	n0.discovery()
		.add_ticket(builder.build(&n0.local().id(), valid_expiry()));
	n1.discovery()
		.add_ticket(builder.build(&n1.local().id(), valid_expiry()));

	timeout_s(5, discover_all([&n0, &n1])).await??;

	let g0 = n0
		.groups()
		.with_key(key)
		.with_state_machine(Counter::default())
		.require_ticket(jwt_validator.clone())
		.join();

	let g1 = n1
		.groups()
		.with_key(key)
		.with_state_machine(Counter::default())
		.require_ticket(jwt_validator.clone())
		.join();

	// Wait for the bond to form between n0 and n1
	timeout_s(T, ensure_bonds_formed(&g0, &n0, &[&n1], "g0")).await?;
	timeout_s(T, ensure_bonds_formed(&g1, &n1, &[&n0], "g1")).await?;

	// Revoke n1's ticket and propagate the updated entry to n0
	n1.discovery().remove_tickets_of(jwt_validator.class());
	n0.discovery().feed(n1.discovery().me());

	// n0's bond to n1 should be terminated because n1 no longer has a valid
	// ticket. We await the next change on n0's bond list and verify it drops.
	timeout_s(T, async {
		loop {
			if g0.bonds().is_empty() {
				break;
			}
			g0.bonds().changed().await;
		}
	})
	.await?;

	assert_eq!(
		g0.bonds().len(),
		0,
		"g0 bond should be gone after revocation"
	);

	Ok(())
}

/// Verifies that `GroupKey::from(&validator)` derives a deterministic key from
/// the validator's signature, and that two nodes using the same validator
/// configuration can form a bond without a manually provided secret.
#[tokio::test]
async fn group_key_derived_from_jwt_validator() -> anyhow::Result<()> {
	const T: u64 = 8;

	let network_id = NetworkId::random();

	let builder = jwt_builder(DEFAULT_ISSUER, DEFAULT_SECRET);
	let jwt_validator = jwt_validator(DEFAULT_ISSUER, DEFAULT_SECRET);

	// Both nodes derive their GroupKey from the same validator — no manual
	// secret involved. The derived key must be identical on both sides.
	let key = GroupKey::from(&jwt_validator);
	let key2 = GroupKey::from(&jwt_validator);
	assert_eq!(
		key, key2,
		"same validator config must produce identical key"
	);

	let (n0, n1) =
		tokio::try_join!(Network::new(network_id), Network::new(network_id))?;

	n0.discovery()
		.add_ticket(builder.build(&n0.local().id(), valid_expiry()));
	n1.discovery()
		.add_ticket(builder.build(&n1.local().id(), valid_expiry()));

	timeout_s(5, discover_all([&n0, &n1])).await??;

	let g0 = n0
		.groups()
		.with_key(key)
		.with_state_machine(Counter::default())
		.require_ticket(jwt_validator.clone())
		.join();

	let g1 = n1
		.groups()
		.with_key(key)
		.with_state_machine(Counter::default())
		.require_ticket(jwt_validator.clone())
		.join();

	assert_eq!(
		g0.id(),
		g1.id(),
		"same key+auth must derive identical group id"
	);

	timeout_s(T, ensure_bonds_formed(&g0, &n0, &[&n1], "g0")).await?;
	timeout_s(T, ensure_bonds_formed(&g1, &n1, &[&n0], "g1")).await?;

	Ok(())
}

/// Verifies that nodes with different auth configurations (different validator
/// signatures) derive different group IDs and never attempt to bond.
#[tokio::test]
async fn mismatched_jwt_auth_config_prevents_bonding() -> anyhow::Result<()> {
	const T: u64 = 5;

	let network_id = NetworkId::random();
	let key = GroupKey::from_secret("auth-mismatch-test-secret");

	let (n0, n1) =
		tokio::try_join!(Network::new(network_id), Network::new(network_id))?;

	timeout_s(T, discover_all([&n0, &n1])).await??;

	let jwt_validator = jwt_validator(DEFAULT_ISSUER, DEFAULT_SECRET);

	// n0 uses the JWT auth validator
	let g0 = n0
		.groups()
		.with_key(key)
		.with_state_machine(Counter::default())
		.require_ticket(jwt_validator)
		.join();

	// n1 joins with no auth — different group ID
	let g1 = n1
		.groups()
		.with_key(key)
		.with_state_machine(Counter::default())
		.join();

	// The two groups should have different IDs
	assert_ne!(
		g0.id(),
		g1.id(),
		"groups with different auth configs should derive different IDs"
	);

	// Allow time for any (erroneous) bonding to occur
	sleep_s(3).await;

	assert_eq!(g0.bonds().len(), 0, "g0 should have no bonds");
	assert_eq!(g1.bonds().len(), 0, "g1 should have no bonds");

	Ok(())
}

/// Verifies that when a group requires multiple ticket validators, peers
/// must satisfy **all** of them to form bonds. Peers that only carry a
/// subset of the required tickets are rejected.
#[tokio::test]
async fn multiple_ticket_validators() -> anyhow::Result<()> {
	const T: u64 = 8;

	let network_id = NetworkId::random();
	let key = GroupKey::from_secret("multi-auth-group-secret");

	// Two independent JWT issuers with different keys and issuer names.
	let builder_a = jwt_builder("issuer-alpha", "secret-alpha");
	let builder_b = jwt_builder("issuer-beta", "secret-beta");

	let validator_a = jwt_validator("issuer-alpha", "secret-alpha");
	let validator_b = jwt_validator("issuer-beta", "secret-beta");

	// n0 and n1 carry tickets from BOTH issuers → should bond.
	// n2 only carries issuer_a's ticket → should be rejected.
	// n3 only carries issuer_b's ticket → should be rejected.
	let (n0, n1, n2, n3) = tokio::try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
	)?;

	n0.discovery()
		.add_ticket(builder_a.build(&n0.local().id(), valid_expiry()));
	n0.discovery()
		.add_ticket(builder_b.build(&n0.local().id(), valid_expiry()));

	n1.discovery()
		.add_ticket(builder_a.build(&n1.local().id(), valid_expiry()));
	n1.discovery()
		.add_ticket(builder_b.build(&n1.local().id(), valid_expiry()));

	n2.discovery()
		.add_ticket(builder_a.build(&n2.local().id(), valid_expiry()));
	// n2 missing issuer_b ticket

	// n3 missing issuer_a ticket
	n3.discovery()
		.add_ticket(builder_b.build(&n3.local().id(), valid_expiry()));

	timeout_s(5, discover_all([&n0, &n1, &n2, &n3])).await??;

	// All nodes join the same group requiring both validators.
	let g0 = n0
		.groups()
		.with_key(key)
		.with_state_machine(Counter::default())
		.require_ticket(validator_a.clone())
		.require_ticket(validator_b.clone())
		.join();

	let g1 = n1
		.groups()
		.with_key(key)
		.with_state_machine(Counter::default())
		.require_ticket(validator_a.clone())
		.require_ticket(validator_b.clone())
		.join();

	let g2 = n2
		.groups()
		.with_key(key)
		.with_state_machine(Counter::default())
		.require_ticket(validator_a.clone())
		.require_ticket(validator_b.clone())
		.join();

	let g3 = n3
		.groups()
		.with_key(key)
		.with_state_machine(Counter::default())
		.require_ticket(validator_a.clone())
		.require_ticket(validator_b.clone())
		.join();

	// n0 and n1 should bond with each other
	timeout_s(T, ensure_bonds_formed(&g0, &n0, &[&n1], "g0")).await?;
	timeout_s(T, ensure_bonds_formed(&g1, &n1, &[&n0], "g1")).await?;

	// Allow time for n2 and n3 to attempt bonding
	sleep_s(3).await;

	let g0_bonds: Vec<_> = g0.bonds().iter().map(|b| *b.peer().id()).collect();
	assert_eq!(g0_bonds.len(), 1, "g0 should have exactly one bond");
	assert!(g0_bonds.contains(&n1.local().id()));

	let g1_bonds: Vec<_> = g1.bonds().iter().map(|b| *b.peer().id()).collect();
	assert_eq!(g1_bonds.len(), 1, "g1 should have exactly one bond");
	assert!(g1_bonds.contains(&n0.local().id()));

	assert_eq!(
		g2.bonds().len(),
		0,
		"g2 should have no bonds (missing issuer_b ticket)"
	);
	assert_eq!(
		g3.bonds().len(),
		0,
		"g3 should have no bonds (missing issuer_a ticket)"
	);

	Ok(())
}
