use mosaik::{
	futures::{SinkExt, StreamExt},
	primitives::{Pretty, Short},
	tdx::{TdxTicket, TdxValidator},
	*,
};

declare::stream!(
	pub SecureConsumer = String,
	"mosaik.examples.tee.tdx.SecureConsumer",
	  consumer require_ticket: TdxValidator::new()
		  .require_mrtd("91eb2b44d141d4ece09f0c75c2c53d247a3c68edd7fafe8a3520c942a604a407de03ae6dc5f87f27428b2538873118b7")
);

declare::collection!(
	pub SecureObservers = mosaik::collections::Map<PeerId, String>,
	"mosaik.examples.tee.tdx.SecureObservers",
	  require_ticket: TdxValidator::new()
		  .require_own_mrtd().expect("TDX support")
		  .require_own_rtmr2().expect("TDX support")
);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	configure_tracing();

	// each build generates a unique network id
	let network_id = format!("mosaik.examples.tee.tdx.{}", env!("BUILD_ID"));
	let network = Network::new(network_id.into()).await?;

	println!("Network {} created...", network.network_id());
	println!("Local peer ID: {}", Short(network.local().id()));

	if network.tdx().available() {
		println!("TDX is available, running consumer flow...");
		consumer_flow(network).await
	} else {
		println!("TDX is not available, running producer flow...");
		producer_flow(network).await
	}
}

async fn consumer_flow(network: Network) -> anyhow::Result<()> {
	println!("Generating TDX ticket...");
	let ticket = network.tdx().ticket()?;
	println!("Generic ticket generated: {:?}", Short(&ticket));

	let tdx_ticket: TdxTicket = ticket.try_into()?;
	println!("TDX ticket: {:#?}", Pretty(&tdx_ticket));

	println!(
		"TDX ticket quote signatures verified: {}",
		tdx_ticket.quote().verify().is_ok()
	);

	// Add our own tdx ticket to our discovery info.
	network.tdx().install_own_ticket()?;
	println!("Installed local TDX ticket into discovery info.");

	println!(
		"Current PeerEntry for local node: {:#?}",
		Pretty(&network.discovery().me())
	);

	// Create a secure stream consumer that requires a TDX ticket satisfying the
	// specified validator criteria for receiving data on this stream.
	let mut consumer = SecureConsumer::consumer(&network);

	// Create a replicated map that is only accessible by peers running in TDX
	// VMs with the same MRTD and RTMR2 measurements as us.
	let observers = SecureObservers::writer(&network);

	println!("Waiting for stream to come online...");
	consumer.when().online().await;

	println!("Waiting for SecureObservers map to come online...");
	observers.when().online().await;

	let my_peer_id = network.local().id();

	loop {
		tokio::select! { biased;
			// When the observers collection is updated, print its current contents.
			() = observers.when().updated() => {
				println!("SecureObservers collection changed:");
				for (peer_id, value) in observers.iter() {
					println!("  {} observed: {value}", Short(peer_id));
				}
				println!();
			}

			Some(item) = consumer.next() => {
				println!("Received stream item: {item}");
				let _ = observers.insert(my_peer_id, item).await;
				println!("SecureObservers collection changed:");
				for (peer_id, value) in observers.iter() {
					println!("  {} observed: {value}", Short(peer_id));
				}
				println!();
			}
		}
	}
}

async fn producer_flow(network: Network) -> anyhow::Result<()> {
	// Stream producers don't need to be running in a TDX environment
	let mut producer = SecureConsumer::producer(&network);

	println!(
		"Current PeerEntry for local node: {:#?}",
		Pretty(&network.discovery().me())
	);

	println!("Waiting for stream to come online...");

	loop {
		producer.when().online().await;
		println!("Producing item on stream...");
		let msg = format!(
			"Hello at timestamp {} from {}",
			std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)?
				.as_secs(),
			Short(network.local().id())
		);
		if let Err(e) = producer.send(msg).await {
			eprintln!("Failed to send message: {:?}", e);
		}
		tokio::time::sleep(std::time::Duration::from_secs(2)).await;
	}
}

fn configure_tracing() {
	use tracing_subscriber::{filter::filter_fn, prelude::*};

	// Avoid re-initializing if a global default subscriber is already set
	if tracing::dispatcher::has_been_set() {
		return;
	}

	let level = tracing::Level::DEBUG;

	// disable noisy modules from dependencies
	let muted: &[&'static str] = &[
		"iroh",
		"rustls",
		"igd_next",
		"hickory_",
		"hyper_util",
		"portmapper",
		"reqwest",
		"events.net.relay.connected",
		"netwatch",
		"mio",
		"acto",
		"swarm_discovery",
		"mainline",
		"pkarr",
		"noq",
	];

	let _ = tracing_subscriber::registry()
		.with(tracing_subscriber::fmt::layer())
		.with(filter_fn(move |metadata| {
			metadata.level() <= &level
				&& !muted
					.iter()
					.any(|prefix| metadata.target().starts_with(prefix))
		}))
		.try_init();

	// Set a custom panic hook that prints and aborts
	let default_hook = std::panic::take_hook();
	std::panic::set_hook(Box::new(move |panic_info| {
		default_hook(panic_info);
		tracing::error!("panic: {panic_info}");
		std::process::abort();
	}));
}
