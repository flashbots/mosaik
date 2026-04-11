# Intel TDX

Intel **Trust Domain Extensions (TDX)** is a hardware-based TEE technology that
provides confidential computing at the virtual machine level. Mosaik's TDX
support lets nodes running inside a TDX Trust Domain generate attestation
tickets and lets other nodes validate them against expected measurements.

Enable TDX support with the `tdx` feature flag:

```toml
[dependencies]
mosaik = { version = "0.3", features = ["tdx"] }
```

## Network Extension

The `tdx` feature adds a `.tdx()` method to `Network` via the `NetworkTdxExt`
trait:

```rust,ignore
use mosaik::{Network, tdx::NetworkTdxExt};

let network = Network::new("my-network".into()).await?;

// Check if TDX hardware is available
if network.tdx().available() {
    // Generate and install an attestation ticket
    network.tdx().install_own_ticket()?;
}
```

### Available Methods

| Method                                        | Description                                                  |
| --------------------------------------------- | ------------------------------------------------------------ |
| `available()`                                 | Returns `true` if TDX hardware is present and functional     |
| `ticket()`                                    | Generates a non-expiring TDX attestation ticket              |
| `ticket_with_expiration(exp)`                 | Generates a ticket with a specific expiration                |
| `install_own_ticket()`                        | Generates a ticket and adds it to the node's discovery entry |
| `measurements()`                              | Returns all TDX measurements (MR_TD, RTMRs)                  |
| `mrtd()`                                      | Returns the MR_TD measurement                                |
| `rtmr0()` / `rtmr1()` / `rtmr2()` / `rtmr3()` | Returns individual RTMR measurements                         |

## TDX Tickets

A `TdxTicket` wraps a hardware-generated TDX Quote together with mosaik-specific
extra data. The ticket structure:

```text
TdxTicket
├── quote_bytes     Raw TDX Quote (hardware-signed attestation)
└── extra
    ├── peer_id     The mosaik PeerId of the attesting node
    ├── network_id  The NetworkId the node belongs to
    ├── started_at  When the node process started
    ├── quoted_at   When the quote was generated
    └── expiration  Ticket expiration policy
```

The TDX Quote's `report_data` field contains a hash of the extra data, creating
a cryptographic binding between the hardware attestation and the mosaik peer
identity.

### Converting between ticket types

`TdxTicket` converts to and from mosaik's generic `Ticket` type:

```rust,ignore
// Generate a TDX-specific ticket
let generic_ticket: Ticket = network.tdx().ticket()?;

// Parse a generic ticket back into a TdxTicket
let tdx_ticket: TdxTicket = generic_ticket.try_into()?;

// Access TDX-specific fields
println!("Peer: {}", tdx_ticket.peer_id());
println!("MR_TD: {}", tdx_ticket.measurements().mrtd());
println!("Quote verified: {}", tdx_ticket.quote().verify().is_ok());
```

Tickets are compressed with zstd before serialization to reduce gossip overhead.

## Measurements

TDX measurements are 48-byte register values that identify the software running
inside a Trust Domain:

| Register | Description                                                 |
| -------- | ----------------------------------------------------------- |
| `MR_TD`  | Measurement of the TD build-time configuration and firmware |
| `RTMR0`  | Runtime measurement register 0                              |
| `RTMR1`  | Runtime measurement register 1                              |
| `RTMR2`  | Runtime measurement register 2                              |
| `RTMR3`  | Runtime measurement register 3 (typically application)      |

### Creating measurements

```rust,ignore
use mosaik::tdx::Measurement;

// From a hex string (96 characters = 48 bytes)
let m = Measurement::hex("91eb2b44d141d4ece09f0c75c2c53d247a3c68edd7fafe8a3520c942a604a407de03ae6dc5f87f27428b2538873118b7");

// From a byte array
let m = Measurement::new([0u8; 48]);

// Parse from string at runtime
let m: Measurement = "91eb2b44...".parse()?;
```

## TdxValidator

`TdxValidator` implements the `TicketValidator` trait for TDX attestation. It
validates that a peer's TDX Quote carries valid hardware signatures and that
the measurements match specified criteria.

### Basic usage

```rust,ignore
use mosaik::tdx::TdxValidator;

// Accept any valid TDX attestation (signatures must verify)
let validator = TdxValidator::new();

// Require a specific MR_TD value
let validator = TdxValidator::new()
    .require_mrtd("91eb2b44d141d4ece09f0c75c2c53d247a3c68edd7fafe8a3520c942a604a407de03ae6dc5f87f27428b2538873118b7");

// Require specific RTMRs
let validator = TdxValidator::new()
    .require_mrtd("...")
    .require_rtmr0("...")
    .require_rtmr2("...");
```

### Builder methods

| Method                       | Description                                      |
| ---------------------------- | ------------------------------------------------ |
| `new()` / `empty()`          | Accept any valid TDX attestation                 |
| `baseline(criteria)`         | Start from a `MeasurementsCriteria` baseline     |
| `require_mrtd(measurement)`  | Require a specific MR_TD value                   |
| `require_rtmr0(measurement)` | Require a specific RTMR0 value                   |
| `require_rtmr1(measurement)` | Require a specific RTMR1 value                   |
| `require_rtmr2(measurement)` | Require a specific RTMR2 value                   |
| `require_rtmr3(measurement)` | Require a specific RTMR3 value                   |
| `allow_variant(criteria)`    | Allow an alternative set of measurement criteria |

### Validation checks

When validating a peer's ticket, `TdxValidator` performs these checks in order:

1. Deserializes the ticket data into a `TdxTicket`
2. Parses and verifies the TDX Quote's hardware signature
3. Verifies the quote's `report_data` matches the hash of the extra data
4. Checks the baseline measurement criteria against the quote's measurements
5. If variant criteria are configured, checks that at least one matches
6. Verifies the ticket's peer ID matches the peer entry's ID
7. Verifies the ticket's network ID matches the peer entry's network ID
8. Checks that the ticket has not expired

### Multiple measurement profiles

The `allow_variant` method lets you accept nodes with different software
configurations. The baseline criteria must always be satisfied, and at least one
variant must also match (if any are configured):

```rust,ignore
use mosaik::tdx::{TdxValidator, MeasurementsCriteria};

let validator = TdxValidator::new()
    // All nodes must have this MR_TD
    .require_mrtd("aabb...")
    // Accept nodes running firmware version A
    .allow_variant(
        MeasurementsCriteria::new().require_rtmr0("1111...")
    )
    // OR firmware version B
    .allow_variant(
        MeasurementsCriteria::new().require_rtmr0("2222...")
    );
```

## Using TDX with Streams

```rust,ignore
use mosaik::*;
use mosaik::tdx::TdxValidator;

// Via the stream! macro
declare::stream!(
    pub SecureFeed = PriceUpdate, "secure.prices",
    consumer require_ticket: TdxValidator::new()
        .require_mrtd("..."),
);

// Via the builder API
let producer = network.streams()
    .producer::<PriceUpdate>()
    .require_ticket(TdxValidator::new().require_mrtd("..."))
    .build()?;

let consumer = network.streams()
    .consumer::<PriceUpdate>()
    .require_ticket(TdxValidator::new().require_mrtd("..."))
    .build();
```

## Using TDX with Groups

```rust,ignore
use mosaik::tdx::TdxValidator;
use mosaik::groups::GroupKey;

let validator = TdxValidator::new().require_mrtd("...");

let group = network.groups()
    .with_key(GroupKey::from(&validator))
    .with_state_machine(MyStateMachine::default())
    .require_ticket(validator)
    .join();
```

## Using TDX with Collections

```rust,ignore
use mosaik::*;
use mosaik::tdx::TdxValidator;

// Via the collection! macro
declare::collection!(
    pub SecureStore = mosaik::collections::Map<String, String>,
    "secure.store",
    require_ticket: TdxValidator::new().require_mrtd("..."),
);

// Via CollectionConfig
let config = CollectionConfig::default()
    .require_ticket(TdxValidator::new().require_mrtd("..."));

let writer = mosaik::collections::Map::<String, String>::writer_with_config(
    &network, store_id, config,
);
```

## TDX Image Builder

Mosaik includes a build-time tool for creating TDX guest images from Rust
crates. The builder cross-compiles your binary for `x86_64-unknown-linux-musl`,
packages it with an Alpine Linux root filesystem, kernel, and OVMF firmware, and
produces a self-extracting launch script.

Enable it in your `build-dependencies`:

```toml
[build-dependencies]
mosaik = { version = "0.3", features = ["tdx-builder-alpine"] }
```

### Usage in build.rs

```rust,ignore
fn main() {
    let output = mosaik::tdx::build::alpine().build();
    for line in format!("{output:#?}").lines() {
        println!("cargo:warning={line}");
    }
}
```

The builder:

1. Cross-compiles the current crate for `x86_64-unknown-linux-musl`
2. Downloads the Alpine Linux minirootfs and a TDX-enabled kernel
3. Obtains OVMF firmware for TDX
4. Bundles everything into a CPIO initramfs
5. Pre-computes the `MR_TD` measurement for the resulting image
6. Generates a self-extracting launch script

The pre-computed `MR_TD` can be used directly in `TdxValidator::require_mrtd()`
for compile-time attestation binding — the binary that builds the image knows
exactly what measurement to expect from the resulting TDX guest.

### Builder options

The `AlpineBuilder` provides a fluent API for customization:

| Method                     | Description                                       |
| -------------------------- | ------------------------------------------------- |
| `alpine_version(m, p)`     | Set Alpine Linux version (default: auto-detected) |
| `ssh_key(key)`             | Add an SSH public key for guest access            |
| `ssh_forward(host, guest)` | Configure SSH port forwarding                     |
| `cpus(n)`                  | Set default vCPU count                            |
| `memory(size)`             | Set default memory (e.g., `"2G"`)                 |
| `extra_file(path, dest)`   | Bundle additional files into the image            |
| `kernel_modules_dir(path)` | Specify custom kernel modules directory           |
| `bundle_runner(bool)`      | Include the QEMU launch script in the output      |
