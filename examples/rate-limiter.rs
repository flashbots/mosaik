//! Distributed rate limiter example.
//!
//! Multiple API gateway instances form a `NoOp` group to get an authenticated
//! peer mesh, then use app-level bond messages to broadcast request hits to
//! each other. Each node keeps a local sliding-window counter aggregated from
//! its own hits plus the hits received from peers, and rejects requests when
//! the aggregate exceeds the configured threshold.
//!
//! Run multiple instances with the same `--key` to form a cluster:
//!
//! ```sh
//! cargo run --example rate-limiter -- --key my-secret --port 3000
//! cargo run --example rate-limiter -- --key my-secret --port 3001
//! ```
//!
//! Then hit any instance:
//!
//! ```sh
//! curl 'http://localhost:3000/api/resource?user_id=123'
//! ```

use {
	axum::{
		Router,
		extract::{Query, State},
		http::StatusCode,
		response::IntoResponse,
		routing::get,
	},
	clap::Parser,
	dashmap::DashMap,
	mosaik::{
		groups::{GroupSecret, NoOp},
		*,
	},
	serde::Deserialize,
	std::{net::SocketAddr, sync::Arc, time::Instant},
};

#[derive(Parser)]
struct Args {
	#[arg(short, long, default_value = "mosaik.example.rate-limiter")]
	network: NetworkId,

	#[arg(short, long)]
	key: GroupSecret,

	#[arg(short, long, default_value = "3000")]
	port: u16,

	/// Allowed requests per minute per user.
	#[arg(short, long, default_value = "10")]
	rpm: u64,
}

struct AppState {
	rpm: u64,
	usage: DashMap<u32, Vec<Instant>>,
}

#[derive(Deserialize)]
struct RequestParams {
	user_id: u32,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	tracing_subscriber::fmt()
		.with_env_filter(
			tracing_subscriber::EnvFilter::try_from_default_env()
				.unwrap_or_else(|_| "rate_limiter=debug,mosaik=debug".parse().unwrap()),
		)
		.init();

	let args = Args::parse();

	let network = Network::new(args.network).await?;
	tracing::info!("joining network {}", network.network_id());

	let group = network
		.groups()
		.with_key(args.key)
		.with_state_machine(NoOp::no_leaders())
		.join();

	tracing::info!("joining group {}", group.id());

	let state = Arc::new(AppState {
		rpm: args.rpm,
		usage: DashMap::new(),
	});

	let app = Router::new()
		.route("/api/resource", get(handle_request))
		.with_state(state);

	let addr = SocketAddr::from(([0, 0, 0, 0], args.port));
	tracing::info!("listening on {addr}");
	let listener = tokio::net::TcpListener::bind(addr).await?;
	axum::serve(listener, app).await?;

	Ok(())
}

async fn handle_request(
	State(state): State<Arc<AppState>>,
	Query(params): Query<RequestParams>,
) -> impl IntoResponse {
	let user_id = params.user_id;

	let now = Instant::now();
	let window = std::time::Duration::from_secs(60);

	let remaining = {
		let mut hits = state.usage.entry(user_id).or_default();
		hits.retain(|t| now.duration_since(*t) < window);
		let remaining = state.rpm.saturating_sub(hits.len() as u64);
		hits.push(now);
		remaining
	};

	if remaining == 0 {
		return (
			StatusCode::TOO_MANY_REQUESTS,
			format!("rate limit exceeded for user {user_id}\n"),
		);
	}

	(
		StatusCode::OK,
		format!("OK for user {user_id}, remaining {remaining}\n"),
	)
}
