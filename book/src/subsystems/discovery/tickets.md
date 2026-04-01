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

A common pattern is to issue JWTs to authorized consumers. The producer
then validates those JWTs inside its `require` predicate.

### 1. Define a ticket class

Both producer and consumer agree on a `UniqueId` for the ticket class:

```rust,ignore
use mosaik::{UniqueId, id};

const JWT_TICKET: UniqueId = id!("my-app.jwt");
```

### 2. Consumer: attach a JWT ticket

The consumer creates a JWT, wraps it in a `Ticket`, and publishes it
via discovery:

```rust,ignore
use mosaik::{Ticket, Bytes};

// (JWT creation uses any standard JWT library)
let jwt_string: String = create_signed_jwt(&consumer_peer_id);

let ticket = Ticket::new(JWT_TICKET, Bytes::from(jwt_string));
network.discovery().add_ticket(ticket);
```

Because tickets propagate through gossip, the producer will see them
when the consumer's updated `PeerEntry` arrives.

### 3. Validate tickets on streams

#### Using `with_ticket_validator` (recommended)

The `with_ticket_validator` method accepts a `TicketValidator`
implementation. It validates tickets at connection time **and**
proactively disconnects peers when their tickets expire:

```rust,ignore
let producer = network.streams()
    .producer::<MyDatum>()
    .with_ticket_validator(MyJwtValidator::new())
    .build()?;

let consumer = network.streams()
    .consumer::<MyDatum>()
    .with_ticket_validator(MyJwtValidator::new())
    .build();
```

Both producer and consumer builders support `with_ticket_validator`.
The `stream!` macro also supports this via the `require_ticket` key:

```rust,ignore
use mosaik::*;

declare::stream!(pub AuthFeed = MyDatum, "auth.feed",
    require_ticket: MyJwtValidator::new(),
);
```

See [Streams > The `stream!` macro](../streams.md#the-stream-macro-recommended)
for full macro syntax.

#### Using `require` with closures

For simpler validation without expiration tracking, use
`has_valid_ticket` inside a `require` predicate:

```rust,ignore
let producer = network.streams()
    .producer::<MyDatum>()
    .require(move |peer| {
        peer.has_valid_ticket(JWT_TICKET, |jwt_bytes| {
            let jwt_str = std::str::from_utf8(jwt_bytes).unwrap_or("");
            // Verify signature, expiration, issuer, subject...
            validate_jwt(jwt_str, peer.id())
        })
    })
    .build()?;
```

The predicate runs each time a consumer attempts to subscribe. If the
consumer has no ticket of the right class, or if every ticket fails the
validator, `has_valid_ticket` returns `false` and the subscription is
rejected with a `NotAllowed` close code.

### 4. What happens at runtime

```
Consumer                        Discovery gossip                    Producer
────────                        ───────────────                     ────────
add_ticket(jwt)  ──────────►  PeerEntry { tickets: [jwt] }
                                        │
                                        ▼
                               gossip / catalog sync
                                        │
                                        ▼
                                                              require(peer)
                                                              └─ has_valid_ticket(...)
                                                                 └─ validate jwt_bytes
                                                                    ├─ valid → accept
                                                                    └─ invalid → reject
```

Consumers that lack a valid ticket are never connected. When using
`with_ticket_validator`, expired tickets trigger **proactive
disconnection** on both producer and consumer sides — no reconnection
attempt is needed. When using the closure-based `require` approach,
expired tickets are rejected on subsequent reconnection attempts.
For groups, expired tickets trigger proactive bond termination —
see [Groups > Joining](../groups/joining.md).



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

| Feature                         | `TicketValidator`       | `require` closure   |
| ------------------------------- | ----------------------- | ------------------- |
| Proactive expiration disconnect | Yes                     | No (next reconnect) |
| Group ID derivation             | Yes (via `signature()`) | Not applicable      |
| Complexity                      | Implement a trait       | Inline closure      |
| Best for                        | Production auth         | Quick prototyping   |

### Example: JWT ticket validator

Below is a complete `TicketValidator` implementation using JWTs with
HMAC-SHA256 signatures. It validates the issuer, subject (must match the
peer's ID), and expiration claims.

Add these dependencies to your `Cargo.toml`:

```toml
[dependencies]
hmac = "0.12"
jwt = "0.16"
sha2 = "0.10"
chrono = "0.4"
```

Then implement the validator:

```rust,ignore
use hmac::{Hmac, digest::KeyInit};
use jwt::{RegisteredClaims, SignWithKey, VerifyWithKey};
use mosaik::{
    Bytes, Expiration, InvalidTicket, PeerId, Ticket,
    TicketValidator, UniqueId, discovery::PeerEntry, id,
};
use std::time::Duration;

pub struct JwtTicketValidator {
    key: Hmac<sha2::Sha256>,
    issuer: String,
    secret: String,
}

impl JwtTicketValidator {
    pub fn new(issuer: &str, secret: &str) -> Self {
        Self {
            key: Hmac::new_from_slice(secret.as_bytes()).unwrap(),
            issuer: issuer.to_string(),
            secret: secret.to_string(),
        }
    }

    /// Create a signed JWT ticket for a specific peer.
    pub fn make_ticket(
        &self,
        peer_id: &PeerId,
        expiration: Expiration,
    ) -> Ticket {
        Ticket::new(
            self.class(),
            jwt::Claims::new(RegisteredClaims {
                issuer: Some(self.issuer.clone()),
                subject: Some(peer_id.to_string()),
                expiration: match expiration {
                    Expiration::Never => None,
                    Expiration::At(ts) => {
                        Some(ts.timestamp() as u64)
                    }
                },
                ..Default::default()
            })
            .sign_with_key(&self.key)
            .unwrap()
            .into(),
        )
    }

    /// Create a ticket that expires after the given duration.
    pub fn make_ticket_expiring_in(
        &self,
        peer_id: &PeerId,
        duration: Duration,
    ) -> Ticket {
        let exp = chrono::Utc::now()
            + chrono::Duration::from_std(duration).unwrap();
        self.make_ticket(peer_id, Expiration::At(exp))
    }
}

impl TicketValidator for JwtTicketValidator {
    fn class(&self) -> UniqueId {
        id!("my-app.jwt")
    }

    fn signature(&self) -> UniqueId {
        // Derive from class + issuer + secret so that validators
        // with different configurations produce different signatures.
        self.class()
            .derive(&self.issuer)
            .derive(&self.secret)
    }

    fn validate(
        &self,
        ticket: &[u8],
        peer: &PeerEntry,
    ) -> Result<Expiration, InvalidTicket> {
        let jwt_str =
            core::str::from_utf8(ticket).map_err(|_| InvalidTicket)?;

        let claims: jwt::Claims = jwt_str
            .verify_with_key(&self.key)
            .map_err(|_| InvalidTicket)?;

        let now = chrono::Utc::now().timestamp() as u64;
        let exp = claims.registered.expiration;

        // Reject if issuer doesn't match
        if claims.registered.issuer.as_deref() != Some(&self.issuer) {
            return Err(InvalidTicket);
        }

        // Reject if subject doesn't match the peer's ID
        if claims.registered.subject
            != Some(peer.id().to_string())
        {
            return Err(InvalidTicket);
        }

        // Reject if already expired
        if exp <= Some(now) {
            return Err(InvalidTicket);
        }

        // Convert the expiration claim to an Expiration value
        Ok(exp
            .and_then(|ts| {
                chrono::DateTime::from_timestamp(ts as i64, 0)
            })
            .map_or(Expiration::Never, Expiration::At))
    }
}
```

### Using the validator

With streams (trait-based):

```rust,ignore
let validator = JwtTicketValidator::new("my-app", "my-secret");

// Producer side -- validates incoming consumers
let producer = network.streams()
    .producer::<MyDatum>()
    .with_ticket_validator(
        JwtTicketValidator::new("my-app", "my-secret"),
    )
    .build()?;

// Consumer side -- validates discovered producers
let consumer = network.streams()
    .consumer::<MyDatum>()
    .with_ticket_validator(
        JwtTicketValidator::new("my-app", "my-secret"),
    )
    .build();
```

With groups:

```rust,ignore
let group = network.groups()
    .builder::<MyStateMachine>()
    .require_ticket(
        JwtTicketValidator::new("my-app", "my-secret"),
    )
    .build()?;
```

The consumer creates and attaches the ticket:

```rust,ignore
let validator = JwtTicketValidator::new("my-app", "my-secret");
let ticket = validator.make_ticket_expiring_in(
    network.local().peer_id(),
    Duration::from_secs(3600),
);
network.discovery().add_ticket(ticket);
```

When the JWT's `exp` claim is reached, the runtime automatically
terminates the connection (stream subscription or group bond) without
waiting for a reconnection attempt.

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

- **Multiple ticket classes.** A single peer can carry tickets of
  different classes simultaneously. Producers can check for any
  combination of classes in their `require` predicate.
