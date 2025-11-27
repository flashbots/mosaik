use {
	core::ops::Range,
	mosaik::prelude::*,
	rblib::alloy::primitives::{Address, BlockHash, U160, U256},
	std::collections::{BTreeMap, HashMap},
};

#[tokio::test]
async fn accumulator_api_design() -> anyhow::Result<()> {
	let network_id = NetworkId::random();

	let n0 = Network::new(network_id.clone()).await?;
	let mut acc_p0_1 = n0
		.produce::<NoncesUpdate>()
		.keyed_by(|n| n.block)
		.accumulate(|acc: &mut HashMap<_, _>, datum| {
			acc.extend(datum.nonces.clone());
		});

	let mut acc_p0_2 = n0
		.produce::<BalancesUpdate>()
		.keyed_by(|b| b.block)
		.accumulate(|acc: &mut HashMap<_, _>, datum| {
			acc.extend(datum.balances.clone());
		});

	let n1 = Network::new(network_id.clone()).await?;
	let mut c1_1 = n1
		.consume::<NoncesUpdate>()
		.keyed_by(|n| n.block)
		.accumulate(|acc: &mut HashMap<_, _>, datum| {
			acc.extend(datum.nonces.clone());
		});

	let mut c1_2 = n1
		.consume::<BalancesUpdate>()
		.keyed_by(|b| b.block)
		.accumulate(|acc: &mut HashMap<_, _>, datum| {
			acc.extend(datum.balances.clone());
		});

	full_manual_disco(&[&n0, &n1]);

	acc_p0_1.status().subscribed().await;
	acc_p0_2.status().subscribed().await;
	c1_1.status().subscribed().await;
	c1_2.status().subscribed().await;
	tracing::info!("Both sides subscribed");

	let nonce_updates = (0..10)
		.map(|i| make_nonces_update(i..(i + 10)))
		.collect::<Vec<_>>();

	let balance_updates = (0..10)
		.map(|i| make_balances_update(i..(i + 10)))
		.collect::<Vec<_>>();

	for (nonces, balances) in nonce_updates.iter().zip(balance_updates.iter()) {
		acc_p0_1.send(nonces.clone()).await?;
		acc_p0_2.send(balances.clone()).await?;
	}

	for update in &nonce_updates {
		let recv_c1_1 = c1_1.next().await;
		let Some(datum) = recv_c1_1 else {
			panic!("Expected datum received None");
		};

		assert_eq!(datum.value(), update);
	}

	for update in &balance_updates {
		let recv_c1_2 = c1_2.next().await;
		let Some(datum) = recv_c1_2 else {
			panic!("Expected datum received None");
		};

		assert_eq!(datum.value(), update);
	}

	tracing::info!("Data received correctly");

	for i in 0..10 {
		let stored = acc_p0_1
			.state()
			.get(&address_of(i))
			.expect("Expected datum to be in accumulator");

		assert_eq!(stored, &nonce_of(i));
	}

	for i in 0..10 {
		let stored = acc_p0_2
			.state()
			.get(&address_of(i))
			.expect("Expected datum to be in accumulator");

		assert_eq!(stored, &balance_of(i));
	}

	tracing::info!("Data stored correctly in producer accumulator");

	let n2 = Network::new(network_id.clone()).await?;
	let mut acc_c2_1 = n2.consume::<NoncesUpdate>().accumulate(
		|acc: &mut HashMap<_, _>, datum| {
			acc.extend(datum.nonces.clone());
		},
	);

	// replay all accumulated state from p0_1 to c2_1
	full_manual_disco(&[&n0, &n1, &n2]);
	acc_c2_1.status().subscribed().await;
	acc_p0_1.status().subscribed().by_at_least(2).await;

	tracing::info!("c2_1 subscribed");

	acc_p0_1.send(make_nonces_update(3..8)).await?;
	tracing::info!("Sent Message #10 from p0_1");

	let recv_c2_1 = acc_c2_1.next().await;
	let Some(datum) = recv_c2_1 else {
		panic!("Expected datum received None");
	};
	tracing::info!("c2_1 received data");
	assert_eq!(datum, make_nonces_update(3..8));
	tracing::info!("Data received correctly on c2_1");
	assert_eq!(
		acc_c2_1
			.state()
			.get(&address_of(5))
			.expect("Expected datum to be in accumulator"),
		&nonce_of(5)
	);
	tracing::info!("Data stored correctly in c2_1 accumulator");
	Ok(())
}

fn full_manual_disco(peers: &[&Network]) {
	for a in peers {
		for b in peers {
			if a.local().id() != b.local().id() {
				a.discovery().catalog().insert(b.local().info());
			}
		}
	}
}

pub type Nonce = u64;
pub type Balance = U256;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoncesUpdate {
	pub block: BlockHash,
	pub nonces: BTreeMap<Address, Nonce>,
}

fn make_nonces_update(accounts: Range<u64>) -> NoncesUpdate {
	let mut nonces = BTreeMap::new();
	let block = U256::from(accounts.start + accounts.end).into();

	for i in accounts {
		let address = U160::from(i).into();
		nonces.insert(address, i + 7);
	}

	NoncesUpdate { block, nonces }
}

fn address_of(i: u64) -> Address {
	U160::from(i).into()
}

fn nonce_of(i: u64) -> Nonce {
	i + 7
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BalancesUpdate {
	pub block: BlockHash,
	pub balances: BTreeMap<Address, Balance>,
}

fn make_balances_update(accounts: Range<u64>) -> BalancesUpdate {
	let mut balances = BTreeMap::new();
	let block = U256::from(accounts.start + accounts.end).into();

	for i in accounts {
		let address = U160::from(i).into();
		balances.insert(address, U256::from(i * 1000));
	}

	BalancesUpdate { block, balances }
}

fn balance_of(i: u64) -> Balance {
	U256::from(i * 1000)
}
