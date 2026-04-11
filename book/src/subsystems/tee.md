# Trusted Execution Environments

Mosaik provides first-class support for **Trusted Execution Environments (TEEs)**,
enabling hardware-attested identity and access control across the network. TEE
support is built on top of the existing [Auth Tickets](discovery/tickets.md)
system — nodes running inside a TEE generate cryptographic attestation tickets
that other nodes can validate before establishing connections.

## Overview

In a TEE-enabled mosaik network, nodes can:

1. **Generate attestation tickets** — a node running inside a TEE produces a
   hardware-signed attestation that proves its software identity (measurements)
   and binds it to its mosaik peer identity and network.
2. **Publish attestation via discovery** — the attestation ticket is attached to
   the node's discovery entry and propagated through gossip, just like any other
   ticket.
3. **Validate peers** — producers, consumers, and groups can require that
   connecting peers carry valid TEE attestation tickets matching specific
   measurement criteria.

This integrates seamlessly with mosaik's existing `TicketValidator` trait and
`require_ticket` API — TEE validators are configured the same way as JWT or
custom validators, but the validation checks hardware-signed attestation quotes
instead of software-signed tokens.

## Supported TEE Platforms

| Platform                                          | Feature flag | Status    |
| ------------------------------------------------- | ------------ | --------- |
| [Intel TDX](tee/tdx.md) (Trust Domain Extensions) | `tdx`        | Supported |
| AMD SEV-SNP                                       | —            | Planned   |
| ARM CCA                                           | —            | Planned   |

## Architecture

```text
┌──────────────────────────────────────────────────────────────┐
│                        TEE Node                              │
│                                                              │
│  ┌────────────────┐    ┌──────────────────────────────────┐  │
│  │  TEE Hardware  │    │         Mosaik Network           │  │
│  │  (TDX / SNP)   │    │                                  │  │
│  │                │    │  Streams ─── require_ticket(v)   │  │
│  │  quote ───────────► │  Groups ──── require_ticket(v)   │  │
│  │  measurements  │    │  Collections─ require_ticket(v)  │  │
│  │                │    │                                  │  │
│  └────────────────┘    └──────────────────────────────────┘  │
│                                                              │
│  attestation ticket ──► Discovery gossip ──► remote peers    │
└──────────────────────────────────────────────────────────────┘
```

## How It Works

### 1. Attestation ticket generation

A node running inside a TEE calls `network.tdx().ticket()` (or the equivalent
for other platforms) to generate an attestation ticket. The ticket contains:

- The raw hardware attestation quote (e.g., a TDX Quote signed by the CPU)
- Extra data binding the attestation to the mosaik peer identity, network, and
  timestamp

The quote's `report_data` field contains a hash of the extra data, so the
hardware cryptographically binds the attestation to the specific peer and network.

### 2. Discovery propagation

The attestation ticket is added to the node's discovery entry via
`network.tdx().install_own_ticket()` and propagated through the normal gossip
and catalog sync mechanisms. No special protocol is needed — TEE attestation
rides on the same infrastructure as JWT tokens and custom credentials.

### 3. Peer validation

Other nodes configure a TEE validator (e.g., `TdxValidator`) on their streams,
groups, or collections. When a new peer is discovered, the validator:

1. Deserializes the attestation ticket from the peer's discovery entry
2. Verifies the hardware signature on the quote
3. Checks that the quote's measurements (MR_TD, RTMRs) match the expected values
4. Verifies that the quote is bound to the correct peer identity and network
5. Returns the ticket's expiration for automatic connection lifecycle management

## Feature Flags

TEE support is gated behind feature flags to avoid pulling in platform-specific
dependencies when not needed:

| Flag                 | Description                                           |
| -------------------- | ----------------------------------------------------- |
| `tee`                | Base TEE support (no platform-specific functionality) |
| `tdx`                | Intel TDX attestation and validation                  |
| `tdx-builder`        | TDX image builder infrastructure                      |
| `tdx-builder-alpine` | Alpine Linux-based TDX image builder                  |
| `tdx-builder-ubuntu` | Ubuntu-based TDX image builder (planned)              |
| `tdx-builder-all`    | All TDX image builders                                |

```toml
[dependencies]
mosaik = { version = "0.3", features = ["tdx"] }

[build-dependencies]
mosaik = { version = "0.3", features = ["tdx-builder-alpine"] }
```

## Quick Example

A stream that only accepts consumers running in a TDX enclave with a specific
firmware measurement:

```rust,ignore
use mosaik::*;
use mosaik::tdx::TdxValidator;

declare::stream!(
    pub SecureStream = MyDatum, "secure.data",
    consumer require_ticket: TdxValidator::new()
        .require_mrtd("abcd...96hex..."),
);

// Producer (outside TEE) — validates consumers
let mut producer = SecureStream::producer(&network);

// Consumer (inside TEE) — generates and installs attestation
network.tdx().install_own_ticket()?;
let mut consumer = SecureStream::consumer(&network);
```

See the [TDX](tee/tdx.md) page for the full API reference and the
[TDX example](https://github.com/flashbots/mosaik/tree/main/examples/tee/tdx)
for a complete working application.
