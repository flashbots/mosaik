# Using TDX Attestations

This tutorial walks through the [TDX example](https://github.com/flashbots/mosaik/tree/main/examples/tee/tdx) — a mosaik application that uses Intel TDX hardware attestation to enforce that only verified enclave guests can subscribe to streams and join replicated collections.

## What Are We Building?

A single binary that runs in two modes depending on the hardware it detects:

- **Producer mode** (regular host) — publishes messages on a stream, validates that every consumer presents a TDX attestation with the correct firmware measurement before accepting the subscription
- **Consumer mode** (inside TDX guest) — generates a hardware-signed attestation ticket, publishes it via discovery, subscribes to the secure stream, and writes received messages into a replicated map shared exclusively with identically-measured TDX peers

```text
┌───────────────────────┐          ┌──────────────────────────────────────┐
│   Producer (host)     │          │  Consumer (TDX guest)                │
│                       │          │                                      │
│  SecureStream         │◄─────────│  TDX Quote + ExtraData               │
│  .producer()          │  gossip  │  .install_own_ticket()               │
│                       │          │                                      │
│  sends messages ──────┼──────────┼─► receives messages                  │
│  (if MR_TD matches)   │   QUIC   │  (validates against declared MRTD)   │
│                       │          │                                      │
│                       │          │  SecureObservers Map (Raft group)    │
│                       │          │  ← only peers with same MRTD+RTMR2 → │
│                       │          │  writes (peer_id, last_message)      │
└───────────────────────┘          └──────────────────────────────────────┘
```

The key insight: mosaik's TEE support integrates directly into the existing `require_ticket` infrastructure. TDX attestation is just another ticket type — you declare it on streams, collections, or groups, and mosaik handles validation automatically.

## Background: TDX Measurements

Intel TDX (Trust Domain Extensions) provides hardware-isolated virtual machines called Trust Domains. Each TD produces a **Quote** — a hardware-signed report containing measurement registers:

| Register | What it measures |
|----------|-----------------|
| **MR_TD** | The firmware/BIOS that booted the TD (OVMF, kernel, initramfs) |
| **RTMR[0]** | Early boot firmware measurements |
| **RTMR[1]** | OS loader and kernel measurements |
| **RTMR[2]** | OS and application-level measurements |
| **RTMR[3]** | Reserved for runtime measurements |

By checking these registers, you can cryptographically verify that a remote peer is running an exact known software stack inside a hardware-protected enclave.

## Project Setup

The TDX example is a workspace member at `examples/tee/tdx/`. It requires the `tdx` feature at runtime and the `tdx-builder-all` feature at build time:

```toml
[dependencies]
mosaik = { version = "0.3", features = ["tdx"] }
tokio = { version = "1", features = ["full"] }
anyhow = "1"

[build-dependencies]
mosaik = { version = "0.3", features = ["tdx-builder-all"] }
```

The `tdx` feature pulls in the TDX runtime APIs (`network.tdx()`, `Tdx` validator, `TdxTicket`). The `tdx-builder-all` feature enables the build-time image builder that creates bootable TDX guest images and pre-computes measurements.

## Step 1: The Build Script

The build script does two things: generates a unique network ID per build, and creates the TDX guest image.

```rust,ignore
fn main() {
    // Unique build ID shared between host and guest binaries
    let build_id = std::env::var("BUILD_ID").unwrap_or_else(|_| {
        let id = mosaik::UniqueId::random().to_string();
        unsafe { std::env::set_var("BUILD_ID", &id) };
        id
    });
    println!("cargo:rustc-env=BUILD_ID={build_id}");

    // Build the TDX guest image
    let build_output = match std::env::var("BUILD_TYPE").as_deref()
    {
        Err(_) | Ok("alpine") => {
            mosaik::tee::tdx::build::alpine().build()
        }
        Ok("ubuntu") => {
            mosaik::tee::tdx::build::ubuntu().build()
        }
        _ => panic!("Unknown BUILD_TYPE"),
    };
}
```

The image builder:
1. Cross-compiles the binary for the guest target (`x86_64-unknown-linux-musl` for Alpine, `x86_64-unknown-linux-gnu` for Ubuntu)
2. Downloads a base rootfs, TDX-compatible kernel, and OVMF firmware
3. Packages everything into a gzipped CPIO initramfs
4. **Pre-computes `MR_TD`, `RTMR[1]`, and `RTMR[2]`** — these are the measurement values you'll use in validators
5. Generates a self-extracting QEMU launch script

The `BUILD_ID` ensures that host and guest binaries from the same build join the same mosaik network, while different builds get isolated networks.

## Step 2: Declare a TDX-Gated Stream

Use the `declare::stream!` macro with `require_ticket` to create a stream that validates consumers:

```rust,ignore
use mosaik::tdx::Tdx;

declare::stream!(
    pub SecureConsumer = String,
    "mosaik.examples.tee.tdx.SecureConsumer",
    consumer require_ticket: Tdx::new()
        .require_mrtd("91eb2b44d141d4ece09f0c75c2c53d247a\
                        3c68edd7fafe8a3520c942a604a407de03\
                        ae6dc5f87f27428b2538873118b7")
);
```

This declares:
- A stream carrying `String` data
- The `consumer require_ticket` clause means the **producer** validates incoming consumers — only consumers whose TDX Quote contains the specified `MR_TD` measurement are allowed to subscribe
- The 96-character hex string is the `MR_TD` value pre-computed by the image builder during the build step

The producer itself does not need to run inside TDX. It just validates that its consumers do.

## Step 3: Declare a TDX-Gated Collection

For the replicated map, use `declare::collection!` with local measurement matching:

```rust,ignore
declare::collection!(
    pub SecureObservers =
        mosaik::collections::Map<PeerId, String>,
    "mosaik.examples.tee.tdx.SecureObservers",
    require_ticket: Tdx::new()
        .require_own_mrtd().expect("TDX support")
        .require_own_rtmr2().expect("TDX support")
);
```

Key difference from the stream: instead of hardcoding measurement values, `require_own_mrtd()` and `require_own_rtmr2()` read the local machine's measurements from TDX hardware at runtime. This means:
- No hex strings to manage
- Only peers running the **exact same firmware and OS configuration** can join the Raft group backing this collection
- Ideal for homogeneous deployments where all TDX guests boot the same image

The `require_ticket` (without `consumer` or `producer` prefix) applies to all group members — every peer must present a valid TDX ticket to participate.

## Step 4: Create the Network

Each build generates a unique network ID so that binaries from the same build discover each other automatically:

```rust,ignore
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let network_id = format!(
        "mosaik.examples.tee.tdx.{}",
        env!("BUILD_ID")
    );
    let network = Network::new(network_id.as_str()).await?;

    println!("Local peer ID: {}", network.local().id());

    if network.tdx().available() {
        consumer_flow(network).await
    } else {
        producer_flow(network).await
    }
}
```

`network.tdx().available()` checks for TDX hardware at runtime. On a regular host, it returns `false` and the binary runs as a producer. Inside a TDX guest VM, it returns `true` and the binary runs as a consumer. Same binary, different behavior.

## Step 5: The Consumer Flow (Inside TDX)

The consumer generates an attestation ticket and uses it to access the secure stream and collection:

```rust,ignore
async fn consumer_flow(
    network: Network,
) -> anyhow::Result<()> {
    // 1. Generate a TDX attestation ticket
    let ticket = network.tdx().ticket()?;

    // 2. Inspect the ticket (optional)
    let tdx_ticket: TdxTicket = ticket.try_into()?;
    println!(
        "Quote verified: {}",
        tdx_ticket.quote().verify().is_ok()
    );

    // 3. Install the ticket into discovery so peers can
    //    validate us
    network.tdx().install_own_ticket()?;

    // 4. Subscribe to the secure stream
    let mut consumer = SecureConsumer::consumer(&network);

    // 5. Open the TDX-gated replicated map
    let observers = SecureObservers::writer(&network);

    // 6. Wait for both to come online
    consumer.when().online().await;
    observers.when().online().await;

    let my_peer_id = network.local().id();

    // 7. Main loop: receive stream items and write them
    //    to the shared map
    loop {
        tokio::select! {
            () = observers.when().updated() => {
                // Another peer wrote to the map
                for (peer_id, value) in observers.iter() {
                    println!("  {peer_id} observed: {value}");
                }
            }
            Some(item) = consumer.next() => {
                println!("Received: {item}");
                let _ = observers
                    .insert(my_peer_id, item)
                    .await;
            }
        }
    }
}
```

The flow in detail:
1. **`network.tdx().ticket()`** — asks the TDX hardware to generate a Quote. The quote's `report_data` is bound to this node's peer ID and network ID, preventing replay attacks.
2. **`install_own_ticket()`** — attaches the ticket to this node's discovery entry. It propagates to other peers via gossip automatically.
3. When the consumer subscribes to the stream, the **producer** checks the consumer's discovery entry for a TDX ticket and validates the `MR_TD` measurement. If it doesn't match, the subscription is rejected.
4. When the consumer opens the `SecureObservers` collection, the underlying Raft group validates all joining peers against the collection's `require_ticket` criteria.

## Step 6: The Producer Flow (Regular Host)

The producer is straightforward — it doesn't need TDX hardware:

```rust,ignore
async fn producer_flow(
    network: Network,
) -> anyhow::Result<()> {
    let mut producer = SecureConsumer::producer(&network);

    loop {
        // Wait for at least one validated consumer
        producer.when().online().await;

        let msg = format!(
            "Hello at timestamp {}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs(),
        );

        if let Err(e) = producer.send(msg).await {
            eprintln!("Failed to send: {e:?}");
        }

        tokio::time::sleep(
            std::time::Duration::from_secs(2),
        ).await;
    }
}
```

The producer publishes every 2 seconds. When a consumer connects, the `require_ticket` validator on the `SecureConsumer` stream automatically checks the consumer's TDX ticket. Invalid consumers are rejected before they receive any data.

## Building

The build compiles both the host binary and the TDX guest image in one step:

```bash
# Alpine image (default — musl, smaller, faster boot)
cargo build -p tdx-example --release

# Ubuntu image (glibc, full package ecosystem)
BUILD_TYPE=ubuntu cargo build -p tdx-example --release
```

After building, you'll find artifacts in `target/release/tdx-artifacts/tdx-example/alpine/` (or `ubuntu/`):

| File | Purpose |
|------|---------|
| `tdx-example-initramfs.cpio.gz` | Bootable initramfs with the binary |
| `tdx-example-vmlinuz` | TDX-compatible kernel |
| `tdx-example-ovmf.fd` | OVMF firmware |
| `tdx-example-run-qemu.sh` | Self-extracting QEMU launch script |
| `tdx-example-mrtd.hex` | Pre-computed MR_TD measurement |
| `tdx-example-rtmr1.hex` | Pre-computed RTMR[1] measurement |
| `tdx-example-rtmr2.hex` | Pre-computed RTMR[2] measurement |

The `.hex` files contain the measurement values you'd use in `require_mrtd(...)` declarations.

## Running

All instances from the same build share a network ID and discover each other automatically via DHT — no manual peer configuration needed.

**Producer (any machine):**

```bash
./target/release/tdx-example
```

If TDX hardware is not present, the binary runs as a producer.

**Consumer (on a TDX-capable host):**

Upload the artifacts and launch the TDX guest:

```bash
# Copy artifacts to TDX host
scp -r target/release/tdx-artifacts/tdx-example/alpine/ \
    user@tdx-host:~/tdx-example

# Launch the TDX guest VM
sudo ./tdx-example/tdx-example-run-qemu.sh
```

You can launch multiple consumer instances — they'll form a Raft group for the `SecureObservers` collection and share state exclusively among peers with matching measurements.

## Validation Patterns

The example demonstrates two complementary validation strategies:

### Hardcoded measurements (stream)

```rust,ignore
Tdx::new().require_mrtd("91eb2b44...")
```

Use when you know the exact image your peers should be running. The measurement value is determined at build time and embedded in the source code. This is the strictest form — only peers booting that exact firmware image pass validation.

### Local measurement matching (collection)

```rust,ignore
Tdx::new()
    .require_own_mrtd().expect("TDX support")
    .require_own_rtmr2().expect("TDX support")
```

Use in homogeneous deployments where all nodes run the same image but you don't want to hardcode hex strings. Each node reads its own measurements from TDX hardware and requires peers to match. This is more convenient for deployment but requires all nodes to boot an identical image.

### Additional options

The `Tdx` validator supports further constraints:

```rust,ignore
// Accept multiple image versions
Tdx::new()
    .require_mrtd("aaa...")       // baseline
    .allow_variant(
        MeasurementsCriteria::new()
            .require_mrtd(Measurement::hex("bbb..."))
    )

// Reject stale attestations
Tdx::new()
    .require_mrtd("aaa...")
    .max_age(Duration::from_secs(3600))

// Require attestation generated after a specific time
Tdx::new()
    .require_mrtd("aaa...")
    .not_before(Utc::now() - Duration::from_secs(86400))
```

## Combining TDX with Other Validators

TDX is not the only ticket type mosaik supports. You can combine TDX attestation with JWT validation, custom `TicketValidator` implementations, or any mix — just add multiple `require_ticket` entries. Peers must satisfy **all** of them to be accepted.

For example, a stream that requires both a valid TDX attestation **and** a JWT signed by a known authority:

```rust,ignore
use mosaik::tdx::Tdx;
use mosaik::tickets::{Jwt, Es256};

declare::stream!(
    pub SecureStream = MyDatum, "secure.data",
    consumer require_ticket: Tdx::new()
        .require_mrtd("91eb2b44..."),
    consumer require_ticket: Jwt::with_key(Es256::hex(
        "0298a82ebe69ad57e0f7d5c2809a..."
    )).allow_issuer("my-auth-service"),
);
```

Or a collection gated by TDX measurements and a custom validator:

```rust,ignore
declare::collection!(
    pub SecureMap =
        mosaik::collections::Map<String, String>,
    "secure.map",
    require_ticket: Tdx::new()
        .require_own_mrtd().expect("TDX support"),
    require_ticket: MyCustomValidator::new(),
);
```

This composability means you can layer hardware attestation on top of application-level authorization. TDX proves the peer is running the right code inside an enclave; JWT (or any other validator) proves the peer is authorized by your identity system. Neither alone is sufficient — both must pass.

## Key Takeaways

1. **TEE attestation is just a ticket** — mosaik's `require_ticket` API treats TDX quotes the same as JWT tokens or custom credentials. No special protocol needed.
2. **Validation is declarative** — use `declare::stream!` or `declare::collection!` with `require_ticket` and the producer or group validates peers automatically.
3. **Build-time measurement pre-computation** — the image builder computes `MR_TD` and `RTMR` values during the build, giving you the exact hex strings to embed in validators.
4. **Local measurement matching** — `require_own_mrtd()` reads measurements from TDX hardware at runtime, avoiding hardcoded values in homogeneous deployments.
5. **Same binary, two modes** — `network.tdx().available()` lets a single binary adapt to its environment, running as producer on regular hosts and consumer inside TDX guests.
6. **Automatic discovery** — TDX tickets propagate via the same gossip and catalog sync as all other discovery data. No separate attestation service required.
