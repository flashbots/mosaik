#![allow(unused)]

mod fut;
mod ticket;
mod time;
mod tracing;

use futures::future::try_join_all;
pub use {
	fut::*,
	mosaik::tickets::{
		EdDsa,
		Es256,
		Es384,
		Hs256,
		Hs384,
		Hs512,
		Jwt,
		JwtTicketBuilder,
		VerifyingKey,
	},
	ticket::*,
	time::*,
};

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
