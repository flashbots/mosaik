use {
	crate::utils::{discover_all, sleep_s, timeout_s},
	mosaik::{
		groups::StateMachine,
		primitives::ShortFmtExt,
		tdx::Tdx,
		tickets::{Es256SigningKey, Expiration, Jwt, JwtTicketBuilder},
		*,
	},
	tokio::try_join,
};

const REQUIRED_MRTD: &str = "91eb2b44d141d4ece09f0c75c2c53d247a3c68edd7fafe8a3520c942a604a407de03ae6dc5f87f27428b2538873118b7";

#[tokio::test]
async fn smoke() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let group_key = GroupKey::random();

	let (n0, n1, n2) = try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
	)?;

	let g0 = n0.groups().with_key(group_key).join();
	let g1 = n1.groups().with_key(group_key).join();
	let g2 = n2.groups().with_key(group_key).join();

	let g0_bonds = g0.bonds();
	let g1_bonds = g1.bonds();
	let g2_bonds = g2.bonds();

	tokio::spawn(async move {
		loop {
			tokio::select! {
				() = g0_bonds.changed() => {
					tracing::info!("g0 bonds changed");
				},
				() = g1_bonds.changed() => {
					tracing::info!("g1 bonds changed");
				},
				() = g2_bonds.changed() => {
					tracing::info!("g2 bonds changed");
				},
			}
		}
	});

	timeout_s(3, discover_all([&n0, &n1, &n2])).await??;

	loop {
		print_bonds(&g0, &n0, "g0");
		print_bonds(&g1, &n1, "g1");
		print_bonds(&g2, &n2, "g2");

		sleep_s(2).await;
	}
}

#[tokio::test]
async fn tdx_attested() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let group_key = GroupKey::random();

	let (n0, n1, n2) = try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
	)?;

	let ticket_v = Tdx::new().require_mrtd(REQUIRED_MRTD);

	let g0 = n0
		.groups()
		.with_key(group_key)
		.require_ticket(ticket_v.clone())
		.join();

	let g1 = n1
		.groups()
		.with_key(group_key)
		.require_ticket(ticket_v.clone())
		.join();

	let g2 = n2
		.groups()
		.with_key(group_key)
		.require_ticket(ticket_v)
		.join();

	timeout_s(3, discover_all([&n0, &n1, &n2])).await??;

	loop {
		print_bonds(&g0, &n0, "g0");
		print_bonds(&g1, &n1, "g1");
		print_bonds(&g2, &n2, "g2");

		sleep_s(2).await;
	}
}

#[tokio::test]
async fn jwt_auth() -> anyhow::Result<()> {
	let network_id = NetworkId::random();
	let group_key = GroupKey::random();

	let (n0, n1, n2) = try_join!(
		Network::new(network_id),
		Network::new(network_id),
		Network::new(network_id),
	)?;

	let jwt_key = Es256SigningKey::random();
	let jwt_pubkey = jwt_key.verifying_key();
	let ticket_b = JwtTicketBuilder::new(jwt_key.clone())
		.issuer("mosaik.tests.groups.p2p.jwt_auth")
		.audience(network_id.to_string())
		.expires_at(Expiration::in_3h());

	n0.discovery().add_ticket(ticket_b.build(&n0.local().id()));
	n1.discovery().add_ticket(ticket_b.build(&n1.local().id()));
	n2.discovery().add_ticket(ticket_b.build(&n2.local().id()));

	let ticket_v = Jwt::with_key(jwt_pubkey)
		.allow_issuer("mosaik.tests.groups.p2p.jwt_auth")
		.allow_audience(network_id.to_string());

	let g0 = n0
		.groups()
		.with_key(group_key)
		.require_ticket(ticket_v.clone())
		.join();

	let g1 = n1
		.groups()
		.with_key(group_key)
		.require_ticket(ticket_v.clone())
		.join();

	let g2 = n2
		.groups()
		.with_key(group_key)
		.require_ticket(ticket_v)
		.join();

	timeout_s(3, discover_all([&n0, &n1, &n2])).await??;

	loop {
		print_bonds(&g0, &n0, "g0");
		print_bonds(&g1, &n1, "g1");
		print_bonds(&g2, &n2, "g2");
		tracing::info!("---");

		sleep_s(2).await;
	}
}

fn print_bonds<M: StateMachine>(group: &Group<M>, local: &Network, name: &str) {
	let bonds = group.bonds();
	if !bonds.is_empty() {
		tracing::info!("{name} bonds (pid = {})", local.local().id().short());
		for bond in bonds.iter() {
			tracing::info!(
				"- {} bonded with {} [bond_id: {}]",
				local.local().id().short(),
				bond.peer().id().short(),
				bond.id().short()
			);
		}
		tracing::info!("");
	}
}
