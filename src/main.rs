#![cfg(feature = "cli")]

//! Mosaik CLI tool
//!
//! This is a command-line interface (CLI) tool for interacting and debugging
//! agents built using the Mosaik networking and streams library. It provides
//! various commands to inspect network status, manage streams, and perform
//! other operations.

use clap::Parser;

mod cli;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	let opts = cli::args::CliOpts::parse();
	cli::tracing::setup(&opts);
	opts.run_command().await
}
