use {
	crate::{
		Criteria,
		PeerId,
		StreamId,
		discovery::PeerEntry,
		primitives::Digest,
	},
	chrono::{DateTime, Utc},
	core::{
		fmt,
		sync::atomic::{AtomicI64, AtomicUsize, Ordering},
		time::Duration,
	},
	humansize::{DECIMAL, format_size},
	humantime::format_duration,
	std::sync::Arc,
	tokio::sync::watch,
};

/// Real-time statistics about an active stream subscription between a consumer
/// and a producer pair.
pub struct Stats {
	/// Timestamp when the last connection between the consumer and the producer
	/// was established. Represented as a unix timestamp in milliseconds.
	///
	/// Notes:
	/// - If the connection with this producer has never been established, or is
	///   currently dropped pending retries, this value is `0`.
	///
	/// - If the connection with this producer was dropped and reestablished,
	///   this value will store the timestamp of the last successful connection.
	connected_at: AtomicI64,

	/// Total number of datums transmitted.
	datums: AtomicUsize,

	/// Total number of bytes transmitted.
	bytes: AtomicUsize,
}

// Public API
impl Stats {
	/// Returns the total number of datums transmitted.
	pub fn datums(&self) -> usize {
		self.datums.load(Ordering::Relaxed)
	}

	/// Returns the total number of bytes transmitted.
	pub fn bytes(&self) -> usize {
		self.bytes.load(Ordering::Relaxed)
	}

	/// Returns the duration since the last successful connection between the
	/// consumer and the remote producer was established.
	pub fn uptime(&self) -> Option<Duration> {
		let ts = self.connected_at.load(Ordering::Relaxed);
		if ts == 0 {
			return None;
		}

		#[allow(clippy::missing_panics_doc)]
		let connected_at = DateTime::<Utc>::from_timestamp_millis(ts).expect(
			"stored connected_at timestamp should always be a valid datetime",
		);

		(Utc::now() - connected_at).to_std().ok()
	}
}

// Internal API
impl Stats {
	pub(crate) fn increment_datums(&self) {
		self.datums.fetch_add(1, Ordering::Relaxed);
	}

	pub(crate) fn increment_bytes(&self, n: usize) {
		self.bytes.fetch_add(n, Ordering::Relaxed);
	}

	pub(crate) fn disconnected(&self) {
		self.connected_at.store(0, Ordering::Relaxed);
	}

	pub(crate) fn connected(&self) {
		self
			.connected_at
			.store(Utc::now().timestamp_millis(), Ordering::Relaxed);
	}

	pub(crate) fn default_connected() -> Self {
		let stats = Self::default();
		stats.connected();
		stats
	}
}

impl Default for Stats {
	fn default() -> Self {
		Self {
			connected_at: AtomicI64::new(0),
			bytes: AtomicUsize::new(0),
			datums: AtomicUsize::new(0),
		}
	}
}

impl core::fmt::Display for Stats {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		write!(
			f,
			"uptime: {}, datums: {}, bytes: {}",
			self
				.uptime()
				.map_or_else(|| "N/A".to_string(), |d| format_duration(d).to_string()),
			self.datums(),
			format_size(self.bytes(), DECIMAL),
		)
	}
}

impl fmt::Debug for Stats {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("Stats")
			.field("uptime", &self.uptime().map(format_duration))
			.field("datums", &self.datums())
			.field("bytes", &format_size(self.bytes(), DECIMAL))
			.finish_non_exhaustive()
	}
}

/// The current connection state between a consumer and a producer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
	/// A connection is in the process of being established with the remote
	/// producer.
	Connecting,

	/// A connection is established between a consumer and a producer.
	Connected,

	/// The connection with the remote producer has been unrecoverably terminated.
	/// Recoverable connection errors that result in retries will not enter this
	/// state, instead go through `Connecting` again until `Connected` is reached
	/// or the retries are exhausted. Optionally carrying the reason for
	/// termination.
	Terminated,
}

/// Represents an active connection between one consumer and one producer.
/// This is used to inspect the real-time state and statistics of the
/// connection.
#[derive(Debug, Clone)]
pub struct ChannelInfo {
	pub(crate) stream_id: StreamId,
	pub(crate) criteria: Criteria,
	pub(crate) producer_id: PeerId,
	pub(crate) consumer_id: PeerId,
	pub(crate) stats: Arc<Stats>,
	pub(crate) peer: Arc<PeerEntry>,
	pub(crate) state: watch::Receiver<State>,
}

// Public API
impl ChannelInfo {
	/// Returns the stream id of the stream this subscription is connected to.
	pub const fn stream_id(&self) -> &StreamId {
		&self.stream_id
	}

	/// Returns the criteria used by the consumer to subscribe to the producer.
	pub const fn criteria(&self) -> &Criteria {
		&self.criteria
	}

	/// Returns the peer id of the producer.
	pub const fn producer_id(&self) -> &PeerId {
		&self.producer_id
	}

	/// Returns the peer id of the consumer.
	pub const fn consumer_id(&self) -> &PeerId {
		&self.consumer_id
	}

	/// Returns the peer entry of the remote peer this subscription is connected
	/// to.
	///
	/// The state of the `PeerEntry` reflects the state of peer at the time the
	/// subscription was established. This may be different from the current
	/// state of the peer in the discovery catalog.
	pub fn peer(&self) -> &PeerEntry {
		&self.peer
	}

	/// Returns the current state of the connection.
	pub fn state(&self) -> State {
		*self.state.borrow()
	}

	/// Returns a watch receiver for monitoring state changes of the connection.
	pub const fn state_watcher(&self) -> &watch::Receiver<State> {
		&self.state
	}

	/// Returns the current snapshot of statistics of the subscription.
	pub fn stats(&self) -> &Stats {
		&self.stats
	}

	/// Returns `true` if the consumer has successfully connected to the producer.
	pub fn is_connected(&self) -> bool {
		self.state() == State::Connected
	}

	/// Returns a future that resolves when the connection is terminated.
	pub fn disconnected(
		&self,
	) -> impl Future<Output = ()> + Send + Sync + 'static {
		let mut state_rx = self.state.clone();
		async move {
			let _ = state_rx.wait_for(|s| s == &State::Terminated).await;
		}
	}
}

/// A map of the latest snapshot of active stream subscriptions.
///
/// This is used on both consumer and producer sides to track active streams.
/// The key of this map is not exposed publicly, but is used internally by
/// consumers and producers to track and map active subscriptions.
pub type ActiveChannelsMap = im::HashMap<Digest, ChannelInfo>;
