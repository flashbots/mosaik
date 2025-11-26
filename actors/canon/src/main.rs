//! An example demonstrating a node that maintains the canonical chain state and
//! updates the p2p network with the latest updates to the chain state.
//!
//! Examples of subscribers to data produced by this node include:
//! - RPC nodes that keep track of updates to signer nonces to filter out stale
//!   transactions.

use {
	core::ops::Range,
	mosaik::prelude::*,
	rblib::{
		alloy::primitives::{U160, U256},
		prelude::*,
	},
	shared::{
		cli::CliNetOpts,
		model::{BalancesUpdate, NoncesUpdate},
		tracing::info,
		*,
	},
	std::collections::BTreeMap,
	tokio::time::interval,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	let opts = CliNetOpts::default();
	info!("Starting canonical chain state actor with options: {opts:#?}");

	if opts.optimism {
		run::<Optimism>(opts).await
	} else {
		run::<Ethereum>(opts).await
	}
}

#[allow(clippy::extra_unused_type_parameters)]
async fn run<P: Platform>(opts: CliNetOpts) -> anyhow::Result<()> {
	let network = Network::new(opts.network_id.clone()).await?;

	// wait for network to be online
	network.local().online().await;

	// connect to bootstrap peers
	for bootstrap in opts.bootstrap {
		info!("Dialing bootstrap peer {bootstrap}");
		network.discovery().dial(bootstrap.into()).await?;
	}

	let mut nonces_tx = network.produce::<NoncesUpdate>();
	let mut balances_tx = network.produce::<BalancesUpdate>();

	// wait for subscribers
	nonces_tx.status().subscribed().await;
	balances_tx.status().subscribed().await;

	let mut counter = 1;
	let mut interval = interval(std::time::Duration::from_secs(2));

	loop {
		tokio::select! {
			_ = interval.tick() => {
				if nonces_tx.status().is_subscribed() && balances_tx.status().is_subscribed() {
					info!("Producing new updates for block {counter}");
					let new_nonces = make_nonces_update(counter..(counter + 10));
					let new_balances = make_balances_update(counter..(counter + 10));

					nonces_tx.send(new_nonces).await?;
					balances_tx.send(new_balances).await?;

					counter += 1;
				}
			}

			() = nonces_tx.status().unsubscribed() => {
				info!("No more subscribers to nonces updates, pausing producer");
			}

			() = balances_tx.status().unsubscribed() => {
				info!("No more subscribers to balances updates, pausing producer");
			}
		}
	}
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

fn make_balances_update(accounts: Range<u64>) -> BalancesUpdate {
	let mut balances = BTreeMap::new();
	let block = U256::from(accounts.start + accounts.end).into();

	for i in accounts {
		let address = U160::from(i).into();
		balances.insert(address, U256::from(i * 1000));
	}

	BalancesUpdate { block, balances }
}
