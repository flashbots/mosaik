use clap::Parser;

#[derive(Debug, Clone, Parser)]
pub struct Opts {
	/// Run in optimism mode
	#[clap(long)]
	pub optimism: bool,
}
