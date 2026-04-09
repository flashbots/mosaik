//! # Mosaik
//!
//! A Rust runtime for building self-organizing, leaderless distributed systems.

pub mod collections;
pub mod discovery;
pub mod groups;
pub mod network;
pub mod primitives;
pub mod streams;

#[cfg(feature = "tee")]
pub mod tee;

#[doc(hidden)]
pub use mosaik_macros::{__collection_impl, __stream_impl, __unique_id_impl};
pub use {
	bytes::{Bytes, BytesMut},
	collections::{
		CollectionReader,
		CollectionWriter,
		ReaderOf,
		StoreId,
		WriterOf,
	},
	futures,
	groups::{
		Consistency,
		Consistency::{Strong, Weak},
		Group,
		GroupId,
		GroupKey,
		LeadershipPreference,
	},
	iroh::{self, SecretKey, Signature},
	network::{Network, NetworkId, PeerId},
	primitives::{Datum, Digest, Tag, Ticket, TicketValidator, UniqueId},
	streams::{
		ConsumerOf,
		Criteria,
		ProducerOf,
		StreamConsumer,
		StreamId,
		StreamProducer,
	},
};

pub mod declare {
	pub use crate::{collection, stream};
}

#[cfg(feature = "tdx")]
pub use tee::{tdx, tdx::NetworkTdxExt};
