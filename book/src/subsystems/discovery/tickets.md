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
inside a producer's `require` predicate.

### Ticket identity

Each ticket has a deterministic `id()` derived from its `class` and
`data`. This ID is used as the key in the peer entry's ticket map, so
adding the same ticket twice is idempotent.

## Creating and attaching tickets

Construct a `Ticket` and add it through the `Discovery` handle:

```rust,ignore
use mosaik::{Ticket, UniqueId, id, Bytes};

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

When you receive a `PeerEntry` (for example, inside an `require`
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

## Example: JWT-authenticated streams

Mosaik ships a built-in [`JwtTicketValidator`] that covers the common
case of HMAC- or RSA-signed JWTs. No extra dependencies or custom trait
implementations needed for the validating side.

### 1. Set up the validator

```rust,ignore
use hmac::{Hmac, digest::KeyInit};
use mosaik::tickets::jwt::JwtTicketValidator;
use sha2::Sha256;

let key: Hmac<Sha256> = Hmac::new_from_slice(b"my-shared-secret").unwrap();

let validator = JwtTicketValidator::with_key(key)
    .allow_issuer("my-app")   // require iss == "my-app"
    .allow_audience("stream"); // require aud == "stream" (optional)
```

Builder methods (all optional, all chainable):

| Method                         | Effect                                                          |
| ------------------------------ | --------------------------------------------------------------- |
| `.allow_issuer(s)`             | Require `iss` to match; call multiple times for OR logic        |
| `.allow_audience(s)`           | Require `aud` to match; call multiple times for OR logic        |
| `.require_subject(s)`          | Override default subject (default: peer id in lowercase hex)    |
| `.require_claim(name, value)`  | Require a custom private claim; multiple calls compose as AND   |
| `.allow_non_expiring()`        | Accept JWTs without an `exp` claim                              |

The validator checks `iss`, `sub`, `nbf`, `exp`, `aud`, and any custom
claims you specify. Tokens with overflowed or non-representable `exp`
timestamps are rejected rather than silently treated as non-expiring.

### 2. Consumer: attach a JWT ticket

The consumer signs a JWT and publishes it via discovery. The ticket
class must be `JwtTicketValidator::CLASS`:

```rust,ignore
use hmac::{Hmac, digest::KeyInit};
use jwt::{Claims, RegisteredClaims, SignWithKey};
use mosaik::{Ticket, tickets::jwt::JwtTicketValidator};
use sha2::Sha256;

let key: Hmac<Sha256> = Hmac::new_from_slice(b"my-shared-secret").unwrap();

let jwt_string = Claims::new(RegisteredClaims {
    issuer: Some("my-app".into()),
    subject: Some(peer_id.to_string().to_lowercase()),
    expiration: Some(
        (chrono::Utc::now() + chrono::Duration::hours(1))
            .timestamp() as u64,
    ),
    ..Default::default()
})
.sign_with_key(&key)
.unwrap();

let ticket = Ticket::new(
    JwtTicketValidator::CLASS,
    jwt_string.into(),
);
network.discovery().add_ticket(ticket);
```

Because tickets propagate through gossip, the producer will see them
when the consumer's updated `PeerEntry` arrives.

### 3. Validate tickets on streams

Use `require_ticket` on either the producer or consumer builder.
It validates at connection time **and** proactively disconnects peers
when their ticket expires. Call it multiple times to require multiple
types of tickets — peers must satisfy all configured validators:

```rust,ignore
let validator = JwtTicketValidator::with_key(key.clone())
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

The `stream!` macro supports this via `require_ticket`:

```rust,ignore
use mosaik::{declare, tickets::jwt::JwtTicketValidator};

declare::stream!(
    pub AuthFeed = MyDatum, "auth.feed",
    require_ticket: JwtTicketValidator::with_key(key).allow_issuer("my-app"),
);
```

See [Streams > The `stream!` macro](../streams.md#the-stream-macro-recommended)
for full macro syntax.

### 4. Validate tickets on groups

```rust,ignore
let validator = JwtTicketValidator::with_key(key.clone())
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
use mosaik::tickets::jwt::JwtTicketValidator;

let producer = network.streams()
    .producer::<MyDatum>()
    .require(move |peer| {
        peer.has_valid_ticket(JwtTicketValidator::CLASS, |jwt_bytes| {
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
                                                         JwtTicketValidator::validate()
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

| Feature                         | Built-in `JwtTicketValidator` | Custom `TicketValidator` | `require` closure   |
| ------------------------------- | ----------------------------- | ------------------------ | ------------------- |
| Proactive expiration disconnect | Yes                           | Yes                      | No (next reconnect) |
| Group ID derivation             | Yes (via `signature()`)       | Yes (via `signature()`)  | Not applicable      |
| Complexity                      | Zero — built in               | Implement a trait        | Inline closure      |
| Best for                        | JWT auth (HMAC / RSA / EC)    | Custom credential types  | Quick prototyping   |

## Custom validators

If you need a credential scheme other than JWTs, implement the
`TicketValidator` trait directly. The three methods to implement:

| Method        | Purpose                                                                                                                    |
| ------------- | -------------------------------------------------------------------------------------------------------------------------- |
| `class()`     | Returns the `UniqueId` matching your ticket class. Only tickets of this class are passed to `validate()`.                  |
| `signature()` | Deterministic ID derived from the validator type **and** config. All group members must produce the same signature.        |
| `validate()`  | Receives raw ticket bytes and the `PeerEntry`. Returns `Ok(Expiration)` on success or `Err(InvalidTicket)` on failure.    |

```rust,ignore
use mosaik::{
    Expiration, InvalidTicket, UniqueId,
    discovery::PeerEntry,
    id,
    tickets::TicketValidator,
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

- **TDX attestation.** Mosaik provides a built-in `TdxValidator` for
  Intel TDX hardware attestation. See
  [TEE > TDX](../tee/tdx.md) for details.
