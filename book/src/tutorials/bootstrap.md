# Building a Bootstrap Node

This tutorial walks through the [bootstrap example](https://github.com/flashbots/mosaik/blob/main/examples/bootstrap.rs) — a ready-to-use bootstrap node for any mosaik network.

## What Is a Bootstrap Node?

A bootstrap node is the first peer that other nodes connect to when joining a network. It serves as the initial discovery point — once a new node connects to a bootstrap peer, the gossip protocol takes over and the joining node learns about all other peers.

Bootstrap nodes are typically:
- **Long-lived** — they run continuously
- **Stable identity** — they use a fixed secret key so their address doesn't change across restarts
- **Well-known** — their address is configured as a bootstrap peer by other nodes

A bootstrap node doesn't need any special code — it's just a regular mosaik node that stays online. The [bootstrap example](https://github.com/flashbots/mosaik/blob/main/examples/bootstrap.rs) can be used in production as a **universal bootstrap node** for any mosaik network. The example adds CLI configuration with `clap`.

## Project Setup

The bootstrap example is a single file at `examples/bootstrap.rs`. It uses these dependencies:

```toml
[dependencies]
mosaik = "0.2"
tokio = { version = "1", features = ["full"] }
clap = { version = "4", features = ["derive"] }
tracing = "0.1"
tracing-subscriber = "0.3"
anyhow = "1"
```

## The CLI Interface

The example uses `clap` derive to define command-line options:

```rust,ignore
use clap::{ArgAction, Parser};
use mosaik::*;

#[derive(Debug, Parser)]
struct Opts {
    /// The secret key for stable identity across restarts
    #[clap(short, long, env = "MOSAIK_BOOTSTRAP_SECRET")]
    secret: Option<SecretKey>,

    /// The network ID (hex string or seed that gets hashed)
    #[clap(short, long, env = "MOSAIK_BOOTSTRAP_NETWORK_ID")]
    network_id: Option<NetworkId>,

    /// Other bootstrap nodes to connect to on startup
    #[clap(short, long, env = "MOSAIK_BOOTSTRAP_PEERS")]
    peers: Vec<PeerId>,

    /// Tags to advertise (default: "bootstrap")
    #[clap(short, long, default_value = "bootstrap",
           env = "MOSAIK_BOOTSTRAP_TAGS")]
    tags: Vec<Tag>,

    /// Disable relay servers (node must be directly reachable)
    #[clap(long, default_value_t = false)]
    no_relay: bool,

    /// Verbose output (-v debug, -vv trace)
    #[clap(short, action = ArgAction::Count)]
    verbose: u8,

    /// Suppress all output
    #[clap(short, long)]
    quiet: bool,
}
```

Every option also supports environment variables (`MOSAIK_BOOTSTRAP_*`), making it easy to configure in containers or systemd services.

## Secret Key Handling

The secret key determines the node's `PeerId`. For a bootstrap node, a stable identity is essential so that other nodes can reliably find it:

```rust,ignore
fn parse_secret_key(s: &str) -> Result<SecretKey, Infallible> {
    let bytes = Digest::from(s);
    Ok(SecretKey::from_bytes(bytes.as_bytes()))
}
```

This parser accepts two formats:
- **64-character hex string** — used directly as the secret key bytes
- **Any other string** — treated as a seed, hashed with blake3 into a deterministic key

So `--secret=my-bootstrap-1` always produces the same key, making deployment reproducible.

## Building the Network

The `main` function assembles the `Network` using the builder:

```rust,ignore
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let opts = Opts::parse();

    let secret = opts.secret.unwrap_or_else(|| {
        tracing::warn!("No secret key provided, generating random key");
        SecretKey::generate(&mut rand::rng())
    });

    let network_id = opts.network_id.unwrap_or_else(|| {
        tracing::warn!("No network id provided, generating random network id");
        NetworkId::random()
    });

    let mut builder = Network::builder(network_id)
        .with_secret_key(secret)
        .with_discovery(
            discovery::Config::builder()
                .with_tags(opts.tags.clone())
                .with_bootstrap(opts.peers.clone()),
        );

    if opts.no_relay {
        builder = builder.with_relay_mode(iroh::RelayMode::Disabled);
    }

    let network = builder.build().await?;

    tracing::info!("Bootstrap node started");
    tracing::info!("Public Id: {}", network.local().id());
    tracing::info!("Network Id: {:?}", network.network_id());

    // Stay alive forever
    core::future::pending::<()>().await;
    Ok(())
}
```

Key points:
1. **`with_secret_key()`** — sets the stable identity
2. **`with_discovery()`** — configures tags and initial peers to dial
3. **`with_relay_mode(Disabled)`** — optional, for nodes with direct connectivity
4. **`core::future::pending()`** — keeps the process alive (the node runs in background tasks)

## Running It

```bash
# Basic usage with a seed-based secret
cargo run --example bootstrap -- \
    --network-id=my-network \
    --secret=my-bootstrap-secret

# With environment variables
MOSAIK_BOOTSTRAP_SECRET=my-secret \
MOSAIK_BOOTSTRAP_NETWORK_ID=my-network \
cargo run --example bootstrap

# Multiple bootstrap nodes that know each other
cargo run --example bootstrap -- \
    --network-id=my-network \
    --secret=node-1 \
    --peers=<peer-id-of-node-2>
```

## Using the Bootstrap Node

Other nodes reference the bootstrap node's address when joining the network:

```rust,ignore
let network = Network::builder(network_id)
    .with_discovery(
        discovery::Config::builder()
            .with_bootstrap(bootstrap_addr)
    )
    .build()
    .await?;
```

The joining node connects to the bootstrap peer, performs a full catalog sync, and then discovers all other nodes through gossip. From that point on, the joining node is a full participant — it doesn't need the bootstrap node for ongoing operation.

## Key Takeaways

1. **Bootstrap nodes are just regular nodes** — no special server code needed
2. **Stable identity via secret key** — ensures the address doesn't change across restarts
3. **Tags for discoverability** — the `"bootstrap"` tag lets other nodes identify bootstrap peers
4. **Minimal configuration** — a secret key and network ID are all that's required
