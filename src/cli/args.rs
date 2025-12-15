use {
	super::{bootstrap, inspect},
	clap::{ArgAction, Parser, Subcommand},
};

#[derive(Parser, Debug)]
#[command(name = "mosaik", about = "Mosaik CLI Tool", version)]
pub struct CliOpts {
	/// Logging verbosity level (-v, -vv, -vvv)
	#[clap(short, long, action = ArgAction::Count, global = true)]
	pub verbose: u8,

	/// Commands
	#[command(subcommand)]
	command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
	/// Create generic bootstrap node
	Bootstrap(bootstrap::Command),
	/// Inspect a peer
	Inspect(inspect::Command),
}

impl CliOpts {
	pub async fn run_command(&self) -> anyhow::Result<()> {
		match &self.command {
			Command::Bootstrap(cmd) => cmd.execute(self).await,
			Command::Inspect(cmd) => cmd.execute(self).await,
		}
	}
}
