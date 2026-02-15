#![allow(unused)]

mod fut;
mod time;
mod tracing;

use futures::future::try_join_all;
pub use {fut::*, time::*};

pub async fn discover_all(
	networks: impl IntoIterator<Item = &mosaik::Network>,
) -> anyhow::Result<()> {
	let networks = networks.into_iter().collect::<Vec<_>>();
	try_join_all(networks.iter().enumerate().flat_map(|(i, net_i)| {
		networks
			.iter()
			.enumerate()
			.filter(move |(j, _)| i != *j)
			.map(move |(_, net_j)| {
				timeout_s(5, net_i.discovery().sync_with(net_j.local().addr()))
			})
	}))
	.await?;
	Ok(())
}
