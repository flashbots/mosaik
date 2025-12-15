//! CLI command to inspect a peer's discovery catalog.
//!
//! This command connects to a specified peer and retrieves its discovery
//! catalog snapshot, displaying the entries in a human-readable format.

use {
	super::args::CliOpts,
	bincode::{
		config::standard,
		serde::{decode_from_slice, encode_to_vec},
	},
	clap::Args,
	futures::{SinkExt, StreamExt},
	mosaik::{PeerId, discovery::SignedPeerEntry, primitives::Pretty},
	serde::{Deserialize, Serialize},
	tokio::io::join,
	tokio_util::codec::{Framed, LengthDelimitedCodec},
};

#[derive(Args, Debug)]
pub struct Command {
	/// Peer Id to inspect
	#[clap(name = "peer-id")]
	pub peer: PeerId,
}

impl Command {
	pub async fn execute(&self, _opts: &CliOpts) -> anyhow::Result<()> {
		let endpoint = iroh::Endpoint::bind().await?;
		let connection = endpoint
			.connect(self.peer, b"/mosaik/discovery/sync/1.0")
			.await?;
		let (tx, rx) = connection.open_bi().await?;

		let mut stream = Framed::new(join(rx, tx), LengthDelimitedCodec::new());
		let sync_message = encode_to_vec(CatalogSnapshot(vec![]), standard())?;
		stream.send(sync_message.into()).await?;

		let response = stream
			.next()
			.await
			.ok_or_else(|| anyhow::anyhow!("no response from peer"))??;

		let catalog_snapshot: CatalogSnapshot =
			decode_from_slice(&response, standard())?.0;

		let own_entry = catalog_snapshot
			.0
			.iter()
			.find(|entry| *entry.id() == self.peer);

		if let Some(entry) = own_entry {
			println!("Peer's own catalog entry:");
			println!("{:?}", Pretty(entry));
			println!();
		} else {
			println!("Peer does not have a catalog entry for itself.");
		}

		println!("Discovery catalog snapshot:");
		for entry in catalog_snapshot.0 {
			println!("{:?}", Pretty(&entry));
			println!();
		}

		Ok(())
	}
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatalogSnapshot(Vec<SignedPeerEntry>);
