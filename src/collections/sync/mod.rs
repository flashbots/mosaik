use {
	crate::{
		PeerId,
		UniqueId,
		groups::{
			Command,
			Cursor,
			Index,
			Message,
			StateMachine,
			StateSync,
			StateSyncContext,
			StateSyncProvider,
			StateSyncSession,
			Term,
		},
	},
	core::{
		any::type_name,
		marker::PhantomData,
		task::{Context, Poll},
	},
	serde::{Deserialize, Serialize, de::DeserializeOwned},
};

mod provider;
mod session;

pub struct SnapshotSync<M: SnapshotStateMachine> {
	config: Config,

	#[doc(hidden)]
	_marker: PhantomData<M>,
}

impl<M: SnapshotStateMachine> Default for SnapshotSync<M> {
	fn default() -> Self {
		Self {
			config: Config::default(),
			_marker: PhantomData,
		}
	}
}

impl<M: SnapshotStateMachine> StateSync for SnapshotSync<M> {
	type Machine = M;
	type Message = SnapshotSyncMessage;
	type Provider = provider::SnapshotSyncProvider<M>;
	type Session = session::SnapshotSyncSession<M>;

	fn signature(&self) -> crate::UniqueId {
		UniqueId::from("mosaik_collections_snapshot_sync").derive(type_name::<M>())
	}

	fn create_provider(&self, cx: &dyn StateSyncContext<Self>) -> Self::Provider {
		provider::SnapshotSyncProvider::new(self.config.clone(), cx)
	}

	fn create_session(
		&self,
		cx: &mut dyn StateSyncContext<Self>,
		position: Cursor,
		leader_commit: Index,
		entries: Vec<(M::Command, Term)>,
	) -> Self::Session {
		todo!()
	}
}

pub(super) trait SnapshotStateMachine: StateMachine {
	type Snapshot: Clone + Send + 'static;
	type Diff: Clone + Send + Serialize + DeserializeOwned + 'static;

	fn snapshot(&self) -> Self::Snapshot;
}

#[derive(Debug, Clone)]
struct Config {
	snapshot_interval: u64,
	window_size: usize,
}

impl Default for Config {
	fn default() -> Self {
		Self {
			snapshot_interval: 1000,
			window_size: 10,
		}
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SnapshotSyncMessage {}
