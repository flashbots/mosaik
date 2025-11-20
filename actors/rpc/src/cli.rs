use mosaik::prelude::*;

#[derive(Debug, Clone, clap::Parser)]
pub struct Opts {
	/// Network ID to connect to.
	#[clap(short, long)]
	pub network_id: NetworkId,

	/// Enable Optimism L2, otherwise Ethereum L1 is used.
	#[clap(long)]
	pub optimism: bool,

	/// Directory to store state. Defaults to a temporary directory.
	#[clap(short, long, default_value_t = default_work_dir())]
	pub data_dir: String,

	/// HTTP JSON-RPC endpoint listen address.
	#[clap(long, default_value = "0.0.0.0:8545")]
	pub listen_addr: String,
}

fn default_work_dir() -> String {
	let path_suffix = format!("rpc-{}", nanoid::nanoid!(6));
	let tempdir = std::env::temp_dir().join(path_suffix);
	tempdir.to_str().expect("invalid temp dir path").to_string()
}
