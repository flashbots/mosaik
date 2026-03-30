# Auth Tickets

Tickets are opaque, typed credentials that peers attach to their discovery
entry. Producers (and other subsystems) can inspect a peer's tickets to
make authorization decisions -- for example, only accepting consumers that
carry a valid JWT.

## Overview

A `Ticket` has two fields:

| Field   | Type       | Description                                                        |
| ------- | ---------- | ------------------------------------------------------------------ |
| `class` | `UniqueId` | Identifies the authorization scheme (e.g. `"my-app.jwt"`)         |
| `data`  | `Bytes`    | Opaque payload whose format is determined by the `class`           |

The discovery system does **not** interpret ticket data. It simply
stores, signs, and propagates tickets alongside the rest of the peer
entry. Validation logic lives entirely in application code -- typically
inside a producer's `accept_if` predicate.

### Ticket identity

Each ticket has a deterministic `id()` derived from its `class` and
`data`. This ID is used as the key in the peer entry's ticket map, so
adding the same ticket twice is idempotent.

## Creating and attaching tickets

Construct a `Ticket` and add it through the `Discovery` handle:

```rust,ignore
use mosaik::{Ticket, UniqueId, unique_id, Bytes};

const MY_TICKET_CLASS: UniqueId = unique_id!("my-app.auth");

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

When you receive a `PeerEntry` (for example, inside an `accept_if`
closure), three methods are available:

| Method                                    | Returns                                          |
| ----------------------------------------- | ------------------------------------------------ |
| `tickets()`                               | `&BTreeMap<UniqueId, Ticket>` -- all tickets     |
| `tickets_of(class)`                       | Iterator over tickets matching the class          |
| `valid_tickets(class, validator)`          | Iterator filtered by a `Fn(&[u8]) -> bool`       |
| `has_valid_ticket(class, validator)`       | `true` if at least one ticket passes the check   |

The `validator` closure receives the raw `data` bytes of each ticket and
returns `true` if the ticket is considered valid.

## Example: JWT-authenticated streams

A common pattern is to issue JWTs to authorized consumers. The producer
then validates those JWTs inside its `accept_if` predicate.

### 1. Define a ticket class

Both producer and consumer agree on a `UniqueId` for the ticket class:

```rust,ignore
use mosaik::{UniqueId, unique_id};

const JWT_TICKET: UniqueId = unique_id!("my-app.jwt");
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

### 3. Producer: validate tickets in `accept_if`

The producer uses `has_valid_ticket` to check incoming consumers:

```rust,ignore
use mosaik::streams;

let producer = network.streams()
    .producer::<MyDatum>()
    .accept_if(move |peer| {
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
                                                              accept_if(peer)
                                                              └─ has_valid_ticket(...)
                                                                 └─ validate jwt_bytes
                                                                    ├─ valid → accept
                                                                    └─ invalid → reject
```

Consumers that lack a valid ticket are never connected. Consumers whose
tickets expire can be rejected on subsequent reconnection attempts (the
predicate is re-evaluated each time).

## Design notes

- **Tickets are public.** They are included in the signed `PeerEntry`
  and propagated to all peers on the network. Do not put secrets in the
  `data` field -- use signed tokens (JWTs, Macaroons) or public-key
  proofs instead.

- **Tickets are not encrypted.** If confidentiality of the credential
  matters, encrypt the payload before wrapping it in a `Ticket` and
  decrypt inside the validator.

- **Ticket validation is synchronous.** The `accept_if` closure and
  the `validator` function passed to `has_valid_ticket` are plain
  `Fn` -- they cannot perform async I/O. Keep validation fast and
  self-contained (e.g. signature checks, expiration comparisons).

- **Multiple ticket classes.** A single peer can carry tickets of
  different classes simultaneously. Producers can check for any
  combination of classes in their `accept_if` predicate.
