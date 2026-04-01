//! TEE Streams — SGX-attested pub/sub demo built with mosaik.
//!
//! Each running instance is one node in the same mosaik network.  Producers
//! only accept subscribers that present a valid SGX attestation ticket, and
//! consumers only subscribe to producers whose attestation matches.  All three
//! nodes in the Docker-compose scenario share the same MRENCLAVE (built from
//! the same binary) and can therefore mutually authenticate.
//!
//! Running natively (outside Gramine):
//!
//!   cargo run -p tee-streams -- --nickname Alice
//!   cargo run -p tee-streams -- --nickname Bob
//!
//! In that mode `read_measurement()` returns `NotInEnclave` and the node
//! falls back to open mode (no attestation), printing a warning.
//!
//! Inside the Docker-compose TEE scenario (`gramine-direct` simulation):
//!
//!   docker compose -f tests/tee/docker-compose-streams.yml up --build

use {
	anyhow::Context,
	clap::Parser,
	futures::SinkExt,
	mosaik::{
		Network,
		NetworkId,
		PeerId,
		StreamId,
		discovery,
		tee::{SgxValidator, make_sgx_ticket, read_measurement},
		unique_id,
	},
	serde::{Deserialize, Serialize},
	std::time::Duration,
	tokio::time,
	tracing::{info, warn},
};

/// A single attestation-protected message.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SecureMessage {
	from: String,
	text: String,
}

/// Shared stream ID — all nodes must use the same value to exchange messages.
const STREAM_ID: StreamId = unique_id!("mosaik.example.tee-streams.messages");
const NETWORK_SALT: NetworkId = unique_id!("mosaik.example.tee-streams");

#[derive(Parser)]
#[command(about = "SGX-attested p2p streams powered by mosaik")]
struct Args {
	/// Display name included in each published message.
	#[arg(short, long, default_value = "anon")]
	nickname: String,

	/// Room name — nodes with the same room join the same mosaik network.
	#[arg(short, long, default_value = "tee-lobby")]
	room: String,

	/// How often to publish a heartbeat message (seconds).
	#[arg(long, default_value = "5")]
	interval: u64,

	/// Optional peer IDs to connect to directly.
	#[arg(short, long)]
	peer: Vec<PeerId>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	tracing_subscriber::fmt()
		.with_env_filter(
			tracing_subscriber::EnvFilter::from_default_env()
				.add_directive("tee_streams=info".parse()?),
		)
		.init();

	let args = Args::parse();

	// ── 1. Read SGX measurement ───────────────────────────────────────────────
	//
	// Try to read this enclave's MRENCLAVE from Gramine's attestation FS.
	// `NotInEnclave` is expected in native dev mode — we continue without
	// attestation (open mode) and log a warning.

	let measurement = match read_measurement() {
		Ok(m) => {
			info!("SGX MRENCLAVE: {m}");
			Some(m)
		}
		Err(mosaik::tee::MeasurementError::NotInEnclave) => {
			warn!(
				"not running inside an SGX enclave — attestation disabled (open mode)"
			);
			None
		}
		Err(e) => return Err(e).context("failed to read SGX measurement"),
	};

	// ── 2. Build the mosaik network ───────────────────────────────────────────

	let network = Network::builder(NETWORK_SALT.derive(args.room.as_str()))
		.with_discovery(discovery::Config::builder().with_bootstrap(args.peer))
		.build()
		.await?;

	info!("local peer id: {}", network.local().id());

	// ── 3. Publish our attestation ticket ─────────────────────────────────────
	//
	// Adding the ticket to the local discovery entry broadcasts it via gossip
	// so that peers can verify our MRENCLAVE before subscribing.

	if let Some(m) = measurement {
		network.discovery().add_ticket(make_sgx_ticket(&m));
		info!("SGX attestation ticket published to discovery");
	}

	// ── 4. Create producer ────────────────────────────────────────────────────
	//
	// Gate subscriptions: only accept consumers that present a valid SGX
	// ticket whose MRENCLAVE matches ours.  In open mode accept everyone.

	let mut producer = {
		let builder = network
			.streams()
			.producer::<SecureMessage>()
			.with_stream_id(STREAM_ID)
			.online_when(|c| c.minimum_of(1));

		if let Some(m) = measurement {
			builder
				.with_ticket_validator(SgxValidator::trusted_measurements([m]))
				.build()?
		} else {
			builder.build()?
		}
	};

	// ── 5. Create consumer ────────────────────────────────────────────────────
	//
	// Only subscribe to producers whose discovery entry carries a valid SGX
	// ticket with the same MRENCLAVE as ours.  In open mode subscribe to all.

	let mut consumer = {
		let builder = network
			.streams()
			.consumer::<SecureMessage>()
			.with_stream_id(STREAM_ID);

		if let Some(m) = measurement {
			builder
				.with_ticket_validator(SgxValidator::trusted_measurements([m]))
				.build()
		} else {
			builder.build()
		}
	};

	info!(
		"started (attestation: {})",
		if measurement.is_some() {
			"on"
		} else {
			"off (open mode)"
		}
	);

	// ── 6. Main loop ──────────────────────────────────────────────────────────

	let nickname = args.nickname.clone();
	let mut interval = time::interval(Duration::from_secs(args.interval));
	interval.tick().await; // skip the immediate first tick

	loop {
		tokio::select! {
			_ = interval.tick() => {
				let msg = SecureMessage {
					from: nickname.clone(),
					text: format!(
						"heartbeat from {} (attested: {})",
						nickname,
						measurement.is_some(),
					),
				};
				match producer.send(msg).await {
					Ok(()) => {}
					// Producer offline (no subscribers yet) — not an error.
					Err(e) => tracing::debug!("send skipped: {e}"),
				}
			}

			Some(msg) = consumer.recv() => {
				info!("[{}] {}", msg.from, msg.text);
			}

			_ = tokio::signal::ctrl_c() => {
				info!("shutting down");
				return Ok(());
			}
		}
	}
}
