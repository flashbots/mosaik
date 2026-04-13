# Auth Tickets

Tickets are opaque, typed credentials that peers attach to their discovery
entry. Producers (and other subsystems) can inspect a peer's tickets to
make authorization decisions -- for example, only accepting consumers that
carry a valid JWT.

## Overview

A `Ticket` has two fields:

| Field   | Type       | Description                                               |
| ------- | ---------- | --------------------------------------------------------- |
| `class` | `UniqueId` | Identifies the authorization scheme (e.g. `"my-app.jwt"`) |
| `data`  | `Bytes`    | Opaque payload whose format is determined by the `class`  |

The discovery system does **not** interpret ticket data. It simply
stores, signs, and propagates tickets alongside the rest of the peer
entry. Validation logic lives entirely in application code -- typically
via a `TicketValidator` implementation.


### Ticket identity

Each ticket has a deterministic `id()` derived from its `class` and
`data`. This ID is used as the key in the peer entry's ticket map, so
adding the same ticket twice is idempotent.

## Creating and attaching tickets

Construct a `Ticket` and add it through the `Discovery` handle:

```rust,ignore
use mosaik::{tickets::Ticket, UniqueId, id, Bytes};

const MY_TICKET_CLASS: UniqueId = id!("my-app.auth");

let ticket = Ticket::new(
    MY_TICKET_CLASS,
    Bytes::from("some-credential-data"),
);

// Attach to this node's discovery entry
network.discovery().add_ticket(ticket);
```

The ticket is immediately included in the next gossip announcement and
catalog sync, so remote peers see it within one announce cycle.

## Removing tickets

```rust,ignore
// Remove a specific ticket by its id
let ticket_id = ticket.id();
network.discovery().remove_ticket(ticket_id);

// Remove all tickets of a given class
network.discovery().remove_tickets_of(MY_TICKET_CLASS);
```

## Querying tickets on a peer entry

When you receive a `PeerEntry` (for example, inside a `require`
closure), five methods are available:

| Method                               | Returns                                                         |
| ------------------------------------ | --------------------------------------------------------------- |
| `tickets()`                          | `&BTreeMap<UniqueId, Ticket>` -- all tickets                    |
| `tickets_of(class)`                  | Iterator over tickets matching the class                        |
| `valid_tickets(class, validator)`    | Iterator filtered by a `Fn(&[u8]) -> bool`                      |
| `has_valid_ticket(class, validator)` | `true` if at least one ticket passes the check                  |
| `validate_ticket(validator)`         | `Result<Expiration, InvalidTicket>` -- longest valid expiration |

The `validator` closure (used by `valid_tickets` and `has_valid_ticket`)
receives the raw `data` bytes of each ticket and returns `true` if valid.
These closure-based methods are convenient for stream `require` predicates.

The `validate_ticket` method accepts a `&dyn TicketValidator` and returns
the `Expiration` with the longest validity among all matching tickets, or
`InvalidTicket` if none pass. This is used internally by the groups
subsystem for expiration-aware bond management.

## Built-in JWT validator

Mosaik ships a built-in [`Jwt`] validator and [`JwtTicketBuilder`] that
cover the common case of JWT-authenticated peers. No external crate
imports are needed.

### Supported algorithms

| Verifying key | Signing key        | Algorithm               |
| ------------- | ------------------ | ----------------------- |
| `Hs256`       | `Hs256`            | HMAC-SHA256 (symmetric) |
| `Hs384`       | `Hs384`            | HMAC-SHA384 (symmetric) |
| `Hs512`       | `Hs512`            | HMAC-SHA512 (symmetric) |
| `Es256`       | `Es256SigningKey`  | ECDSA P-256             |
| `Es256k`      | `Es256kSigningKey` | ECDSA secp256k1         |
| `Es384`       | `Es384SigningKey`  | ECDSA P-384             |
| `Es512`       | `Es512SigningKey`  | ECDSA P-521             |
| `EdDsa`       | `EdDsaSigningKey`  | Ed25519                 |

For HMAC algorithms the same key type is used for both signing and
verification (symmetric). For ECDSA and EdDSA the signing key is a
private key and the verifying key is the corresponding public key.

All types live in `mosaik::tickets`.

### 1. Set up the validator (HMAC)

```rust,ignore
use mosaik::tickets::{Jwt, Hs256};

let validator = Jwt::with_key(Hs256::hex(
        "87e50edd90e2f9d53a7f2a9bd51c1069a454b827f0e1002577c54e1c2a598568"
    ))
    .allow_issuer("my-app")
    .allow_audience("stream");
```

### 1b. Set up the validator (ECDSA)

```rust,ignore
use mosaik::tickets::{Jwt, Es256};

// Compressed P-256 public key (33 bytes, SEC1 format)
let validator = Jwt::with_key(Es256::hex(
        "0298a82ebe69ad57e0f7d5c2809a05188ff58572c5a5009015f26643f91e0d3236"
    ))
    .allow_issuer("my-app");
```

Validator builder methods (all optional, all chainable):

| Method                        | Effect                                                        |
| ----------------------------- | ------------------------------------------------------------- |
| `.allow_issuer(s)`            | Require `iss` to match; call multiple times for OR logic      |
| `.allow_audience(s)`          | Require `aud` to match; call multiple times for OR logic      |
| `.require_subject(s)`         | Override default subject (default: peer id in lowercase hex)  |
| `.require_claim(name, value)` | Require a custom private claim; multiple calls compose as AND |
| `.allow_non_expiring()`       | Accept JWTs without an `exp` claim                            |
| `.max_lifetime(duration)`     | Reject tokens with `exp - now` exceeding this duration        |
| `.max_age(duration)`          | Require `iat` and reject tokens older than this duration      |

The validator checks `iss`, `sub`, `nbf`, `exp`, `aud`, `iat`, and any
custom claims you specify. Tokens with overflowed or non-representable
`exp` timestamps are rejected rather than silently treated as
non-expiring.

### 2. Issue tickets with `JwtTicketBuilder`

`JwtTicketBuilder` is the issuing counterpart to `Jwt`. It signs JWTs
and wraps them as `Ticket` values ready for discovery:

```rust,ignore
use mosaik::tickets::{Hs256, JwtTicketBuilder};

let builder = JwtTicketBuilder::new(Hs256::new(secret))
    .issuer("my-app");

let ticket = builder.build(&peer_id, Expiration::At(expiration_time));
network.discovery().add_ticket(ticket);
```

For asymmetric algorithms, use the signing key type:

```rust,ignore
use mosaik::tickets::{Es256SigningKey, JwtTicketBuilder};

let sk = Es256SigningKey::random();

let builder = JwtTicketBuilder::new(sk)
    .issuer("my-app");

let ticket = builder.build(&peer_id, Expiration::At(expiration_time));
network.discovery().add_ticket(ticket);
```

Builder methods (all optional, all chainable):

| Method                  | Effect                                                   |
| ----------------------- | -------------------------------------------------------- |
| `.issuer(s)`            | Set the `iss` claim                                      |
| `.subject(s)`           | Override the `sub` claim (default: peer id in lower hex) |
| `.audience(s)`          | Set the `aud` claim                                      |
| `.issued_at(datetime)`  | Override `iat` (default: current time)                   |
| `.not_before(datetime)` | Set the `nbf` claim                                      |
| `.token_id(jti)`        | Set the `jti` claim                                      |
| `.claim(name, value)`   | Add a custom claim (standard names route to their field) |

Because tickets propagate through gossip, the remote peer will see them
when the updated `PeerEntry` arrives.

### 3. Validate tickets on streams

Use `require_ticket` on either the producer or consumer builder.
It validates at connection time **and** proactively disconnects peers
when their ticket expires. Call it multiple times to require multiple
types of tickets — peers must satisfy all configured validators:

```rust,ignore
use mosaik::tickets::{Jwt, Hs256};

let validator = Jwt::with_key(Hs256::new(secret))
    .allow_issuer("my-app");

// Producer side — validates incoming consumers
let producer = network.streams()
    .producer::<MyDatum>()
    .require_ticket(validator.clone())
    .build()?;

// Consumer side — validates discovered producers
let consumer = network.streams()
    .consumer::<MyDatum>()
    .require_ticket(validator.clone())
    .build();
```

The `stream!` macro supports `require_ticket`, enabling compile-time
ACL declarations. Combined with `Es256::hex`, a stream's access
policy — including the exact public key — is fully expressed in code
with no runtime configuration:

```rust,ignore
use mosaik::{declare, tickets::{Jwt, Es256}};

declare::stream!(
    pub AuthFeed = MyDatum, "auth.feed",
    producer require_ticket: Jwt::with_key(Es256::hex(
        "0298a82ebe69ad57e0f7d5c2809a05188ff58572c5a5009015f26643f91e0d3236"
    )).allow_issuer("my-auth-service"),
);
```

Here `producer require_ticket` means consumers validate producer
tickets — only producers that present a JWT signed by the
corresponding private key are accepted. The hex-encoded P-256 public
key and issuer constraint are baked into the binary at compile time.

HMAC keys work the same way:

```rust,ignore
use mosaik::{declare, tickets::{Jwt, Hs256}};

declare::stream!(
    pub AuthFeed = MyDatum, "auth.feed",
    require_ticket: Jwt::with_key(Hs256::hex(
        "87e50edd90e2f9d53a7f2a9bd51c1069a454b827f0e1002577c54e1c2a598568"
    )).allow_issuer("my-app"),
);
```

See [Streams > The `stream!` macro](../streams.md#the-stream-macro-recommended)
for full macro syntax.

### 4. Validate tickets on groups

```rust,ignore
use mosaik::tickets::{Jwt, Hs256};

let validator = Jwt::with_key(Hs256::new(secret))
    .allow_issuer("my-app");

let group = network.groups()
    .with_key(GroupKey::from(&validator))
    .with_state_machine(MyStateMachine::default())
    .require_ticket(validator)
    .join();
```

`GroupKey::from(&validator)` derives the group identity from the
validator's `signature()`, so all nodes using the same validator
configuration automatically derive the same group key.

### 5. Using `require` closures (alternative)

For quick prototyping without expiration tracking, use `has_valid_ticket`
inside a `require` predicate:

```rust,ignore
use mosaik::tickets::Jwt;

let producer = network.streams()
    .producer::<MyDatum>()
    .require(move |peer| {
        peer.has_valid_ticket(Jwt::CLASS, |jwt_bytes| {
            let jwt_str = std::str::from_utf8(jwt_bytes).unwrap_or("");
            // Custom signature check...
            verify_jwt(jwt_str, peer.id())
        })
    })
    .build()?;
```

The predicate runs each time a consumer attempts to subscribe. Expired
tickets are only caught on the next reconnection attempt (no proactive
disconnect).

### 6. What happens at runtime

```text
Consumer                        Discovery gossip              Producer / Group
────────                        ───────────────               ────────────────
sign JWT + add_ticket() ─────► PeerEntry { tickets: [jwt] }
                                        │
                                        ▼
                               gossip / catalog sync
                                        │
                                        ▼
                                                         Jwt::validate()
                                                         ├─ sig ok, iss ok, sub ok
                                                         ├─ nbf ≤ now ≤ exp
                                                         ├─ valid → accept / bond
                                                         └─ invalid → reject
```

When using `require_ticket`, the runtime
schedules a disconnect at the moment `exp` is reached — no reconnection
round-trip required. For groups, expired tickets trigger proactive bond
termination — see [Groups > Joining](../groups/joining.md).

## The `TicketValidator` trait

The `TicketValidator` trait provides structured, expiration-aware ticket
validation. Unlike the closure-based `require` approach (which returns a
simple `bool`), a `TicketValidator` returns an `Expiration` value that
the runtime uses to **proactively disconnect** peers when their
credentials expire -- no reconnection round-trip required.

```rust,ignore
pub trait TicketValidator: Send + Sync + 'static {
    /// Class of tickets that this validator can validate.
    fn class(&self) -> UniqueId;

    /// A unique identifier for the validator type and its configuration.
    ///
    /// In Groups, this is used as part of the group ID derivation, so
    /// all members must use the same validator type and configuration.
    fn signature(&self) -> UniqueId;

    /// Validates the given ticket for the specified peer.
    fn validate(
        &self,
        ticket: &[u8],
        peer: &PeerEntry,
    ) -> Result<Expiration, InvalidTicket>;
}
```

### Method summary

| Method        | Purpose                                                                                                                                                                         |
| ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `class()`     | Returns the `UniqueId` matching the ticket class this validator handles. Only tickets of this class are passed to `validate()`.                                                 |
| `signature()` | Returns a deterministic identifier derived from the validator type **and** its configuration. Used in group ID derivation -- all group members must produce the same signature. |
| `validate()`  | Receives the raw ticket bytes and the `PeerEntry` of the peer being validated. Returns `Ok(Expiration)` on success or `Err(InvalidTicket)` on failure.                          |

### When to use `TicketValidator` vs `require` closures

| Feature                         | Built-in `Jwt`                  | Custom `TicketValidator` | `require` closure   |
| ------------------------------- | ------------------------------- | ------------------------ | ------------------- |
| Proactive expiration disconnect | Yes                             | Yes                      | No (next reconnect) |
| Group ID derivation             | Yes (via `signature()`)         | Yes (via `signature()`)  | Not applicable      |
| Complexity                      | Zero — built in                 | Implement a trait        | Inline closure      |
| Best for                        | JWT auth (HMAC / ECDSA / EdDSA) | Custom credential types  | Quick prototyping   |

## Custom validators

If you need a credential scheme other than JWTs, implement the
`TicketValidator` trait directly:

```rust,ignore
use mosaik::{
    UniqueId,
    discovery::PeerEntry,
    id,
    tickets::{Expiration, InvalidTicket, TicketValidator},
};

pub struct MyValidator {
    // your config fields
}

impl TicketValidator for MyValidator {
    fn class(&self) -> UniqueId {
        id!("my-app.credential")
    }

    fn signature(&self) -> UniqueId {
        // Derive from class + any config that affects validation logic.
        // Two validators with different configs must produce different
        // signatures to avoid mismatched group IDs.
        self.class().derive("v1")
    }

    fn validate(
        &self,
        ticket: &[u8],
        peer: &PeerEntry,
    ) -> Result<Expiration, InvalidTicket> {
        // Verify the credential bytes against `peer`.
        // Return Ok(Expiration::At(ts)) for time-limited credentials,
        // Ok(Expiration::Never) for permanent ones.
        todo!()
    }
}
```

Plug it in exactly like the built-in validator:

```rust,ignore
let producer = network.streams()
    .producer::<MyDatum>()
    .require_ticket(MyValidator { /* ... */ })
    .build()?;
```

## TDX hardware attestation

Mosaik provides a built-in `Tdx` ticket validator for Intel TDX
(Trust Domain Extensions) hardware attestation. TDX tickets contain
a hardware-signed quote that cryptographically proves the code running
inside the trust domain, allowing peers to verify each other's
integrity without trusting the host operating system.

```rust,ignore
use mosaik::tickets::Tdx;

let validator = Tdx::new()
    .require_mrtd("a1b2c3d4...")   // expected MR_TD measurement
    .require_rtmr0("e5f6a7b8..."); // expected RTMR0

let group = network.groups()
    .with_key(GroupKey::from(&validator))
    .with_state_machine(MyMachine::default())
    .require_ticket(validator)
    .join();
```

TDX tickets can be combined with JWT tickets by calling
`require_ticket` multiple times — peers must satisfy **all** validators.

See [TEE > TDX](../tee/tdx.md) for the full TDX API, measurement
types, image builders, and multi-variant support.

## Design notes

- **Tickets are public.** They are included in the signed `PeerEntry`
  and propagated to all peers on the network. Do not put secrets in the
  `data` field -- use signed tokens (JWTs, Macaroons) or public-key
  proofs instead.

- **Tickets are not encrypted.** If confidentiality of the credential
  matters, encrypt the payload before wrapping it in a `Ticket` and
  decrypt inside the validator.

- **Ticket validation is synchronous.** The `require` closure and
  the `validator` function passed to `has_valid_ticket` are plain
  `Fn` -- they cannot perform async I/O. The `TicketValidator::validate`
  method is also synchronous. Keep validation fast and self-contained
  (e.g. signature checks, expiration comparisons).

- **Expiration-aware validation.** `TicketValidator::validate` returns
  `Result<Expiration, InvalidTicket>`. Validators should return
  `Expiration::At(time)` for time-limited credentials (e.g. JWTs with
  `exp` claims) and `Expiration::Never` for permanent ones. Both the
  groups and streams subsystems use this to proactively terminate
  connections when tickets expire, rather than waiting for the next
  reconnection attempt.

- **Multiple validators.** All subsystems (streams, groups, collections)
  support multiple `require_ticket` calls. Peers must satisfy **all**
  configured validators to be accepted. The runtime tracks the earliest
  expiration across all validators for automatic disconnect scheduling.

- **Multiple ticket classes.** A single peer can carry tickets of
  different classes simultaneously. Producers can check for any
  combination of classes in their `require` predicate.

- **TDX attestation.** Mosaik provides a built-in `Tdx` validator for
  Intel TDX hardware attestation. See
  [TEE > TDX](../tee/tdx.md) for details.
