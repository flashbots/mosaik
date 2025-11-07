use {
	anyhow::Result,
	clap::Parser,
	core::{fmt, str::FromStr},
	futures::TryStreamExt,
	iroh::{
		Endpoint,
		EndpointAddr,
		EndpointId,
		protocol::{AcceptError, ProtocolHandler, Router},
	},
	iroh_gossip::{
		Gossip,
		TopicId,
		api::{Event, GossipReceiver},
	},
	serde::{Deserialize, Serialize},
	sha3::Digest,
	tracing::info,
};

#[derive(Parser, Debug)]
struct Opts {
	#[clap(long)]
	pub peers: Vec<EndpointId>,

	#[clap(long)]
	pub ticket: Option<Ticket>,

	#[clap(long, default_value = "mosaik-default")]
	pub topic_id: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	init_logging();

	let opts = Opts::parse();
	let endpoint = Endpoint::bind().await?;

	endpoint.addr();

	let topic_id = sha3::Sha3_256::digest(opts.topic_id.as_bytes());
	let topic_id = TopicId::from_bytes(topic_id.into());

	info!("Local endpoint id: {}", endpoint.id());
	info!("Peers: {:?}", opts.peers);
	info!("Topic id: {topic_id}");

	let ticket = Ticket {
		topic: topic_id,
		endpoints: vec![endpoint.addr()],
	};
	info!("Ticket: {ticket}");

	let gossip = Gossip::builder().spawn(endpoint.clone());

	let router = Router::builder(endpoint.clone())
		.accept(iroh_gossip::ALPN, gossip.clone())
		.accept(NetworkDef::ALPN, NetworkDef)
		.spawn();

	let (gossip_tx, gossip_rx) = if let Some(ticket) = opts.ticket {
		info!("Joining topic from ticket: {ticket}");
		gossip
			.subscribe_and_join(
				ticket.topic,
				ticket.endpoints.into_iter().map(|e| e.id).collect(),
			)
			.await?
	} else {
		gossip.subscribe_and_join(topic_id, opts.peers).await?
	}
	.split();

	gossip_tx
		.broadcast(format!("Hello everyone, I am {}!", endpoint.id()).into())
		.await?;

	let (line_tx, mut line_rx) = tokio::sync::mpsc::channel(1);

	tokio::spawn(input_loop(line_tx, endpoint.clone()));
	tokio::spawn(output_loop(gossip_rx));

	while let Some(text) = line_rx.recv().await {
		gossip_tx.broadcast(text.into()).await?;
	}

	router.shutdown().await?;

	Ok(())
}

#[derive(Debug)]
struct NetworkDef;

impl NetworkDef {
	pub const ALPN: &'static [u8] = b"/mosaik/net-def/1";
}

impl ProtocolHandler for NetworkDef {
	async fn accept(
		&self,
		connection: iroh::endpoint::Connection,
	) -> Result<(), AcceptError> {
		let (_tx, _rx) = connection.accept_bi().await?;

		info!(
			"[network-def] Accepted connection from {}",
			connection.remote_id()?
		);
		Ok(())
	}
}

async fn input_loop(
	tx: tokio::sync::mpsc::Sender<String>,
	endpoint: Endpoint,
) -> anyhow::Result<()> {
	let me = endpoint.id();
	let mut buffer = String::new();
	while std::io::stdin().read_line(&mut buffer).is_ok() {
		let text = buffer.trim();
		if text.is_empty() {
			buffer.clear();
			continue;
		}
		let message = format!("{me}: {text}");
		let result = tx.send(message.clone()).await;
		info!("[gossip] Broadcasting message: {message} [{result:?}]");
		buffer.clear();
	}

	Ok(())
}

async fn output_loop(mut rx: GossipReceiver) -> anyhow::Result<()> {
	while let Some(event) = rx.try_next().await? {
		info!("[gossip] Event: {event:?}");
		let neighbors = rx.neighbors().collect::<Vec<_>>();
		info!("[gossip] Current neighbors: {neighbors:?}");

		match event {
			Event::NeighborUp(public_key) => {
				info!("[gossip] Neighbor up: {public_key}");
			}
			Event::NeighborDown(public_key) => {
				info!("[gossip] Neighbor down: {public_key}");
			}
			Event::Received(message) => {
				let text = String::from_utf8_lossy(&message.content);
				info!(
					"[gossip] Received message from {} (scope={:?}): {}",
					message.delivered_from, message.scope, text
				);
			}
			Event::Lagged => {
				info!("[gossip] Lagged event");
			}
		}
	}

	Ok(())
}

fn init_logging() {
	let _ = tracing_subscriber::fmt()
		.with_env_filter(
			tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(
				|_| tracing_subscriber::EnvFilter::new("info,iroh=info,iroh_*=info"),
			),
		)
		.with_target(false)
		.compact()
		.try_init();
}

// add the `Ticket` code to the bottom of the main file
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Ticket {
	topic: TopicId,
	endpoints: Vec<EndpointAddr>,
}

impl Ticket {
	/// Deserialize from a slice of bytes to a Ticket.
	fn from_bytes(bytes: &[u8]) -> Result<Self> {
		rmp_serde::from_slice(bytes).map_err(Into::into)
	}

	/// Serialize from a `Ticket` to a `Vec` of bytes.
	pub fn to_bytes(&self) -> Vec<u8> {
		rmp_serde::to_vec(self).expect("rmp_serde::to_vec is infallible")
	}
}

// The `Display` trait allows us to use the `to_string`
// method on `Ticket`.
impl fmt::Display for Ticket {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		let mut text = data_encoding::BASE32_NOPAD.encode(&self.to_bytes()[..]);
		text.make_ascii_lowercase();
		write!(f, "{text}")
	}
}

// The `FromStr` trait allows us to turn a `str` into
// a `Ticket`
impl FromStr for Ticket {
	type Err = anyhow::Error;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		let bytes =
			data_encoding::BASE32_NOPAD.decode(s.to_ascii_uppercase().as_bytes())?;
		Self::from_bytes(&bytes)
	}
}
