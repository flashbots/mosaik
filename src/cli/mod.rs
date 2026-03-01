use clap::{Parser, Subcommand};

pub mod dht;

#[derive(Debug, Parser)]
pub struct CliOpts {
	#[clap(subcommand)]
	pub command: CliCommand,
}

impl CliOpts {
	pub async fn run(&self) -> anyhow::Result<()> {
		match &self.command {
			CliCommand::Dht(cmd) => cmd.run(self).await,
		}
	}
}

#[derive(Debug, Subcommand)]
pub enum CliCommand {
	/// Run the peer discovery service
	Dht(dht::DhtCommand),
}
