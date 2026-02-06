use {
	super::{
		super::{Datum, StreamId},
		builder::ProducerConfig,
		worker::{Handle, WorkerLoop},
	},
	crate::{discovery::Discovery, network::LocalNode},
	dashmap::{DashMap, Entry},
	derive_more::{Deref, From, Into},
	std::sync::Arc,
};

/// Responsible for keeping track of all active stream producers' fanout sinks.
///
/// All producers of the same stream type share a single sink instance to
/// fan out data to remote consumers.
///
/// This type is cheap to clone and all clones point to the same underlying map.
pub(in crate::streams) struct Sinks {
	/// The local node instance of the network associated with this streams
	/// subsystem.
	pub local: LocalNode,

	/// The discovery system used to announce newly created streams.
	pub discovery: Discovery,

	/// Map of all active fanout sinks by stream id that have producers
	/// associated with them.
	pub active: Arc<DashMap<StreamId, SinkHandle>>,
}

impl Sinks {
	/// Creates a new empty Sinks map.
	pub(in crate::streams) fn new(
		local: LocalNode,
		discovery: Discovery,
	) -> Self {
		Self {
			local,
			discovery,
			active: Arc::new(DashMap::new()),
		}
	}
}

impl Sinks {
	/// Given a stream id, opens or creates the shared fanout sink for that
	/// stream. If no sink for this stream id exists, a new one is created with
	/// the builder's configuration. Otherwise an error is returned containing
	/// the existing sink created using the original configuration.
	pub fn create<D: Datum>(
		&self,
		config: ProducerConfig,
	) -> Result<SinkHandle, SinkHandle> {
		let stream_id = config.stream_id;
		match self.active.entry(stream_id) {
			Entry::Vacant(entry) => {
				// Create a new fanout sink worker loop for this stream id
				// and insert it into the active map, then return a handle to it.
				let sink = WorkerLoop::<D>::spawn(self, config);
				let handle = entry.insert(sink.into()).clone();

				// Update our local peer entry in discovery to include this stream id
				// in the list of advertised streams so it can be discovered by others.
				self
					.discovery
					.update_local_entry(move |me| me.add_streams(stream_id));

				Ok(handle)
			}
			Entry::Occupied(entry) => Err(entry.get().clone()),
		}
	}

	/// Opens an existing fanout sink for the given stream id, if it exists.
	pub fn open(&self, stream_id: StreamId) -> Option<SinkHandle> {
		self.active.get(&stream_id).map(|entry| entry.clone())
	}
}

impl core::fmt::Debug for Sinks {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		write!(f, "Sinks({} streams)", self.active.len())
	}
}

/// A handle to a stream sink for a specific stream id.
#[derive(Clone, Deref, From, Into)]
pub(in crate::streams) struct SinkHandle(Arc<Handle>);

impl From<Handle> for SinkHandle {
	fn from(handle: Handle) -> Self {
		Self(Arc::new(handle))
	}
}
