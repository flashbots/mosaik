#![cfg_attr(docsrs, feature(doc_cfg))]

//! # Mosaik
//!
//! A Rust runtime for building self-organizing, leaderless distributed
//! systems. Mosaik handles peer discovery, connectivity, consensus, and
//! replicated state so you can focus on application logic.
//!
//! Built on [iroh](https://docs.rs/iroh) (QUIC-based peer-to-peer transport
//! with relay support), mosaik nodes find each other automatically through
//! gossip and DHT, form groups with Raft consensus, and synchronize data
//! through typed streams and replicated collections — all without a central
//! coordinator.
//!
//! For tutorials, architecture guides, and worked examples, see the
//! [Mosaik Book](https://docs.mosaik.rs).
//!
//! # Getting started
//!
//! Every mosaik application starts by creating a [`Network`]. Nodes that
//! share the same [`NetworkId`] discover each other automatically:
//!
//! ```rust
//! use mosaik::*;
//!
//! let network = Network::new("my-network-id").await?;
//! ```
//!
//! # Subsystems
//!
//! Mosaik is organized into four subsystems, each accessible from a
//! [`Network`] instance:
//!
//! ## Discovery
//!
//! The [`discovery`] subsystem handles automatic peer finding through
//! gossip and DHT. Nodes announce their presence, the streams they
//! produce, and the groups they belong to. Other nodes learn about them
//! without any manual configuration.
//!
//! ## Streams
//!
//! The [`streams`] subsystem provides typed, async pub/sub channels. A
//! [`Producer`](streams::Producer) publishes data, and any number of
//! [`Consumer`](streams::Consumer)s on the network can subscribe. Streams
//! implement [`futures::Sink`] and [`futures::Stream`], so they plug
//! directly into the async ecosystem:
//!
//! ```rust
//! // Open a producer for a stream of strings
//! let mut producer = network.streams().produce::<String>();
//!
//! // Wait until at least one consumer subscribes
//! producer.when().subscribed().await;
//!
//! // Send data
//! producer.send("hello".to_string()).await?;
//! ```
//!
//! Use the [`stream!`](declare::stream) macro to declare streams at
//! compile time with baked-in configuration:
//!
//! ```rust
//! mosaik::stream!(pub Telemetry = SensorReading,
//!     online_when: subscribed().minimum_of(1),
//! );
//! ```
//!
//! ## Groups
//!
//! The [`groups`] subsystem provides consensus groups — clusters of
//! trusted nodes that coordinate through a modified Raft consensus
//! protocol. Groups elect a leader, replicate a command log, and apply
//! entries to a pluggable [`StateMachine`](groups::StateMachine):
//!
//! ```rust
//! let group = network.groups().with_key("my-group-key").join();
//!
//! group.when().leader_elected().await;
//! ```
//!
//! ## Collections
//!
//! The [`collections`] subsystem offers replicated data structures that
//! are built on top of groups. Each collection is backed by its own
//! Raft group, providing strong consistency for mutations:
//!
//! - [`Map<K,V>`](collections::Map) — key-value store
//! - [`Vec<T>`](collections::Vec) — ordered, append-friendly list
//! - [`Set<T>`](collections::Set) — unique-element set
//! - [`Cell<T>`](collections::Cell) — single replicated value
//! - [`Once<T>`](collections::Once) — write-once value
//! - [`PriorityQueue<P,K,V>`](collections::PriorityQueue) — priority queue
//!
//! Each collection has a **writer** (mutates via Raft) and a **reader**
//! (read-only replica):
//!
//! ```rust
//! let scores = collections::Map::<String, u64>::writer(&network, "leaderboard");
//!
//! scores.when().online().await;
//! scores.insert("alice".into(), 42).await?;
//! ```
//!
//! Use the [`collection!`](declare::collection) macro for compile-time
//! declarations:
//!
//! ```rust
//! mosaik::collection!(pub Leaderboard =
//!     collections::Map<String, u64>, "leaderboard");
//! ```
//!
//! # Trusted Execution Environments
//!
//! The optional [`tee`] module (enabled with the `tee` feature) adds
//! support for running mosaik nodes inside hardware-isolated enclaves.
//! Currently supported:
//!
//! - **Intel TDX** (`tdx` feature) — nodes running inside a TDX Trust Domain
//!   can generate attestation quotes that prove their identity and code
//!   integrity. These quotes are used as [`Ticket`]s so that streams and
//!   collections can gate access to verified TEE peers only.
//!
//! The `tdx-builder-alpine` and `tdx-builder-ubuntu` features provide
//! build-time image builders for packaging Rust crates into bootable
//! TDX guest images.
//!
//! # Reactive conditions
//!
//! All major types expose a `.when()` builder that returns a future
//! resolving when a topology or consensus condition is met:
//!
//! ```rust,ignore
//! // Wait for a specific number of subscribers
//! producer.when().subscribed().minimum_of(3).await;
//!
//! // Wait for a collection mutation to replicate
//! let ver = scores.insert("bob".into(), 99).await?;
//! reader.when().reaches(ver).await;
//!
//! // Wait for group leader election
//! group.when().leader_elected().await;
//! ```

pub mod collections;
pub mod discovery;
pub mod groups;
pub mod network;
pub mod primitives;
pub mod streams;
pub mod tickets;

#[cfg(feature = "tee")]
pub mod tee;

#[cfg_attr(docsrs, doc(hidden))]
pub use bytes::{Bytes, BytesMut};
#[cfg_attr(docsrs, doc(hidden))]
pub use collections::{
	CollectionConfig,
	CollectionReader,
	CollectionWriter,
	ReaderOf,
	StoreId,
	WriterOf,
};
#[cfg_attr(docsrs, doc(hidden))]
pub use futures;
#[cfg_attr(docsrs, doc(hidden))]
pub use groups::{
	Consistency,
	Consistency::{Strong, Weak},
	Group,
	GroupId,
	GroupKey,
};
#[cfg_attr(docsrs, doc(hidden))]
pub use iroh::{self, SecretKey, Signature};
#[cfg_attr(docsrs, doc(hidden))]
pub use mosaik_macros::{__collection_impl, __stream_impl, __unique_id_impl};
#[cfg_attr(docsrs, doc(hidden))]
pub use network::{Network, NetworkId, PeerId};
#[cfg_attr(docsrs, doc(hidden))]
pub use primitives::{Datum, Digest, Tag, UniqueId};
#[cfg_attr(docsrs, doc(hidden))]
pub use streams::{
	ConsumerOf,
	Criteria,
	ProducerOf,
	StreamConsumer,
	StreamId,
	StreamProducer,
};

/// Compile-time declaration macros for streams and collections.
///
/// Use [`stream!`] to declare typed pub/sub channels and
/// [`collection!`] to declare replicated data structures with
/// baked-in identifiers and configuration. See their respective
/// module docs for full syntax.
pub mod declare {
	pub use crate::{collection, stream};
}

#[cfg_attr(docsrs, doc(hidden))]
#[cfg(feature = "tdx")]
pub use tee::{tdx, tdx::NetworkTdxExt};
