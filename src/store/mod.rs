//! Synchronized Data Store protocol
//!
//! This module provides functionality for replaying streams of data for nodes
//! joining the network after data has already been produced. It allows new
//! consumers to catch up on past data produced by producers, ensuring they have
//! access to the complete data history.
//!
//! Notes:
//!
//! - In this iteration of the replay module, only producers replay state for
//!   new consumers joining the network. Consumers do not replay state to other
//!   consumers.

use {
	crate::{
		Group,
		discovery::Discovery,
		network::LocalNode,
		primitives::UniqueId,
		store::worker::WorkerLoop,
	},
	std::sync::Arc,
	worker::Handle,
};

mod config;
mod worker;

pub use config::{Config, ConfigBuilder, ConfigBuilderError};
use iroh::protocol::RouterBuilder;

#[derive(Clone)]
#[expect(dead_code)]
pub struct Stores(Arc<Handle>);

impl Stores {
	pub fn new(local: LocalNode, discovery: &Discovery, config: Config) -> Self {
		Self(WorkerLoop::spawn(local, discovery.clone(), config))
	}

	pub fn primary(&self, _group: &Group, _id: StoreId) -> PrimaryStore {
		PrimaryStore {}
	}

	pub fn replica(&self, _id: StoreId) -> ReplicaStore {
		ReplicaStore {}
	}
}

// Add all gossip ALPNs to the protocol router
impl crate::network::ProtocolProvider for Stores {
	fn install(&self, protocols: RouterBuilder) -> RouterBuilder {
		protocols
	}
}

/// A unique identifier for a store within the Mosaik network.
pub type StoreId = UniqueId;

pub struct PrimaryStore {}

pub struct ReplicaStore {}
