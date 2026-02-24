# Installation

## Requirements

- **Rust ≥ 1.89** (edition 2024)
- A Unix-like OS (Linux, macOS) or Windows

## Add to Your Project

Add mosaik to your `Cargo.toml`:

```toml
[dependencies]
mosaik = "0.2"
```

Mosaik pulls in its core dependencies automatically, including:

- [`tokio`](https://docs.rs/tokio) — async runtime (full features)
- [`iroh`](https://docs.rs/iroh) — QUIC-based P2P networking
- [`serde`](https://docs.rs/serde) — serialization framework
- [`futures`](https://docs.rs/futures) — `Stream` and `Sink` traits

## Common Additional Dependencies

Most mosaik applications will also want:

```toml
[dependencies]
tokio = { version = "1", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
futures = "0.3"
anyhow = "1"
```

- **`tokio`** — you need the tokio runtime to run mosaik
- **`serde` with `derive`** — for `#[derive(Serialize, Deserialize)]` on your data types
- **`futures`** — for `StreamExt` / `SinkExt` traits on consumers and producers

## Verify Installation

Create a minimal test to verify everything links correctly:

```rust,ignore
use mosaik::{Network, NetworkId};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let network = Network::new("test-network".into()).await?;
    println!("Node online: {}", network.local().id());
    Ok(())
}
```

```bash
cargo run
```

If you see a node ID printed, mosaik is installed and working.

## Feature Flags

Mosaik currently does not define any optional feature flags. All functionality is included by default.
