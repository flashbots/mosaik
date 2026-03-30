//! Group Chat — a peer-to-peer chat room built with mosaik.
//!
//! Each running instance is one participant. Start several to chat:
//!
//!   cargo run -p group-chat -- --nickname Alice
//!   cargo run -p group-chat -- --nickname Bob --color 33
//!   cargo run -p group-chat -- --nickname Charlie --color 34
//!
//! Participants in the same room automatically discover each other via
//! mosaik's built-in gossip and DHT. No central server needed.

use {
	clap::Parser,
	mosaik::{Network, NetworkId, StoreId, collections, unique_id},
	serde::{Deserialize, Serialize},
	std::{io::BufRead, time::Duration},
	tokio::sync::mpsc,
};

/// Shared store IDs — all participants must use these exact IDs to join the
/// same replicated collections.
const NETWORK_SALT: NetworkId = unique_id!("mosaik.example.group-chat");
const USERS: StoreId = unique_id!("mosaik.example.group-chat.users");
const MESSAGES: StoreId = unique_id!("mosaik.example.group-chat.messages");

/// Info stored per connected participant.
#[derive(Clone, Serialize, Deserialize)]
struct UserInfo {
	nickname: String,
	color: u8,
}

/// A single chat message.
#[derive(Clone, Serialize, Deserialize)]
struct Message {
	from: String,
	color: u8,
	text: String,
}

#[derive(Parser)]
#[command(about = "p2p group chat powered by mosaik")]
struct Args {
	/// Your display name.
	#[arg(short, long, default_value = "anon")]
	nickname: String,

	/// ANSI color for your messages
	/// (31=red 32=green 33=yellow 34=blue 35=magenta 36=cyan).
	#[arg(short, long, default_value = "32")]
	color: u8,

	/// Room name — everyone with the same room name joins the same chat.
	#[arg(short, long, default_value = "lobby")]
	room: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	let args = Args::parse();

	// Each room is its own mosaik network, derived from the hardcoded network id
	// salt and the room name.
	let network = Network::new(NETWORK_SALT.derive(args.room.as_str())).await?;

	println!(
		"Joining room '{}' as \x1b[{}m{}\x1b[0m ...",
		args.room, args.color, args.nickname
	);

	// Two replicated collections shared across all participants.
	//   users    — Map<node-id, UserInfo>   who is in the room
	//   messages — Vec<Message>             chat history in append order
	let users = collections::Map::<String, UserInfo>::writer(&network, USERS);
	let messages = collections::Vec::<Message>::writer(&network, MESSAGES);

	// Block until we've joined both collection groups and caught up with peers.
	users.when().online().await;
	messages.when().online().await;

	// Register ourselves so others can see us in the participant list.
	users
		.insert(network.local().id().to_string(), UserInfo {
			nickname: args.nickname.clone(),
			color: args.color,
		})
		.await
		.ok();

	// Print current participants and any existing message history.
	println!("--- {} participant(s) ---", users.len());
	for (_, u) in users.iter() {
		println!("  \x1b[{}m{}\x1b[0m", u.color, u.nickname);
	}
	let history: Vec<_> = messages.iter().collect();
	if !history.is_empty() {
		println!("--- {} message(s) in history ---", history.len());
		for m in &history {
			print_msg(m);
		}
	}
	println!("--- start typing, Ctrl-C to quit ---\n");

	// Read stdin lines in a background thread and funnel them through a channel
	// so the async loop can select on them without blocking.
	let (tx, mut rx) = mpsc::unbounded_channel::<String>();
	std::thread::spawn(move || {
		for line in std::io::stdin().lock().lines().map_while(Result::ok) {
			if tx.send(line).is_err() {
				break;
			}
		}
	});

	let mut displayed = history.len();

	loop {
		// Wait for a typed message or a short timeout to refresh the display.
		tokio::select! {
			Some(text) = rx.recv() => {
				let m = Message {
					from: args.nickname.clone(),
					color: args.color,
					text,
				};
				messages.push_back(m).await.ok();
			}
			() = tokio::time::sleep(Duration::from_millis(100)) => {}
		}

		// Print any messages that have arrived (from us or others) since last tick.
		let all: Vec<_> = messages.iter().collect();
		for m in all.iter().skip(displayed) {
			print_msg(m);
		}
		displayed = all.len();
	}
}

fn print_msg(m: &Message) {
	println!("\x1b[{}m{}\x1b[0m: {}", m.color, m.from, m.text);
}
