use {
	super::PeerEntry,
	crate::network::PeerId,
	core::{ops::Deref, time::Duration},
	dashmap::DashMap,
	iroh::endpoint::Connection,
	std::time::Instant,
};

/// Per-peer smoothed RTT state following RFC 6298 SRTT/RTTVAR.
///
/// The smoothing algorithm:
/// - First sample: `smoothed = sample`, `variance = sample / 2`
/// - Subsequent: `variance = 3/4 * variance + 1/4 * |smoothed - sample|`, then
///   `smoothed = 7/8 * smoothed + 1/8 * sample`
struct RttState {
	/// Smoothed RTT (SRTT)
	smoothed: Duration,

	/// RTT variance (RTTVAR)
	variance: Duration,

	/// Number of samples incorporated
	sample_count: u64,

	/// Timestamp of last sample
	last_sample: Instant,
}

/// A smoothed RTT estimate for a peer.
pub type Rtt = Duration;

/// Thread-safe per-peer RTT tracker using EWMA smoothing.
///
/// This is an internal component of the discovery system. It is never
/// exposed through the public API — users access RTT exclusively
/// through [`PeerInfo`] in `require()` predicates and config options.
pub struct RttTracker {
	state: DashMap<PeerId, RttState>,
}

impl core::fmt::Debug for RttTracker {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		f.debug_struct("RttTracker")
			.field("peers_tracked", &self.state.len())
			.finish()
	}
}

impl RttTracker {
	/// Creates a new empty tracker.
	pub fn new() -> Self {
		Self {
			state: DashMap::new(),
		}
	}

	/// Records a new RTT sample for the given peer, updating the
	/// EWMA estimate.
	pub fn record_sample(&self, peer_id: PeerId, rtt: Duration) {
		use dashmap::Entry;

		match self.state.entry(peer_id) {
			Entry::Occupied(mut entry) => {
				let state = entry.get_mut();
				// RFC 6298 update rules:
				// RTTVAR = (1 - β) * RTTVAR + β * |SRTT - R|
				// SRTT   = (1 - α) * SRTT   + α * R
				// where α = 1/8, β = 1/4
				let diff = state.smoothed.abs_diff(rtt);
				state.variance =
					mul_duration(state.variance, 3, 4) + mul_duration(diff, 1, 4);
				state.smoothed =
					mul_duration(state.smoothed, 7, 8) + mul_duration(rtt, 1, 8);
				state.sample_count += 1;
				state.last_sample = Instant::now();
			}
			Entry::Vacant(entry) => {
				// First sample: SRTT = R, RTTVAR = R/2
				entry.insert(RttState {
					smoothed: rtt,
					variance: rtt / 2,
					sample_count: 1,
					last_sample: Instant::now(),
				});
			}
		}
	}

	/// Returns the current smoothed RTT estimate for the given peer.
	pub fn get(&self, peer_id: &PeerId) -> Option<Rtt> {
		self.state.get(peer_id).map(|entry| entry.value().smoothed)
	}

	/// Removes RTT data for a departed peer.
	pub fn remove(&self, peer_id: &PeerId) {
		self.state.remove(peer_id);
	}
}

/// Returns the RTT of the currently selected (active) network path for
/// a connection.
///
/// A connection can have multiple paths (relay, direct IPv4, direct IPv6).
/// `PathId::ZERO` is always the relay path from the initial handshake.
/// After holepunching succeeds, the selected path switches to the direct
/// path which has a more accurate RTT. This function always returns the
/// RTT of whichever path is currently selected for transmission.
pub fn best_rtt(connection: &Connection) -> Option<Duration> {
	connection
		.paths()
		.into_iter()
		.find(|p| p.is_selected())
		.and_then(|p| p.rtt())
}

/// A view of a peer combining its self-reported [`PeerEntry`] with
/// locally-observed metrics like RTT.
///
/// This type implements `Deref<Target = PeerEntry>` so existing code
/// that accesses `peer.tags()`, `peer.streams()`, etc. continues to
/// work unchanged through auto-deref.
///
/// # RTT semantics
///
/// - `rtt()` returns `None` for peers without RTT measurements (e.g. newly
///   discovered peers that have not been contacted yet).
/// - `rtt_below(threshold)` returns `true` when RTT is unknown (optimistic
///   admission — peers are not excluded until there is evidence they are too
///   slow).
#[derive(Debug, Clone)]
pub struct PeerInfo {
	entry: PeerEntry,
	rtt: Option<Rtt>,
}

impl PeerInfo {
	/// Constructs a `PeerInfo` from a peer entry and an RTT tracker,
	/// looking up the current RTT for the peer automatically.
	pub(crate) fn from_tracker(entry: &PeerEntry, tracker: &RttTracker) -> Self {
		let rtt = tracker.get(entry.id());
		Self {
			entry: entry.clone(),
			rtt,
		}
	}

	/// Returns the smoothed RTT to this peer, if measurements are
	/// available.
	pub const fn rtt(&self) -> Option<Rtt> {
		self.rtt
	}

	/// Returns `true` if the smoothed RTT is below the given threshold.
	///
	/// Returns `true` if no RTT data is available (optimistic — peers
	/// are not excluded until there is evidence they are too slow).
	pub fn rtt_below(&self, threshold: Duration) -> bool {
		self.rtt().is_none_or(|rtt| rtt < threshold)
	}

	/// Returns the inner [`PeerEntry`].
	pub fn into_entry(self) -> PeerEntry {
		self.entry
	}
}

impl Deref for PeerInfo {
	type Target = PeerEntry;

	fn deref(&self) -> &PeerEntry {
		&self.entry
	}
}

impl From<PeerInfo> for PeerEntry {
	fn from(info: PeerInfo) -> Self {
		info.entry
	}
}

impl AsRef<PeerEntry> for PeerInfo {
	fn as_ref(&self) -> &PeerEntry {
		&self.entry
	}
}

/// Multiplies a `Duration` by a rational number `num/den`.
fn mul_duration(d: Duration, num: u32, den: u32) -> Duration {
	d * num / den
}

#[cfg(test)]
mod tests {
	use super::*;

	fn test_peer_id() -> PeerId {
		let secret = iroh::SecretKey::generate(&mut rand::rng());
		secret.public()
	}

	#[test]
	fn first_sample_initializes_state() {
		let tracker = RttTracker::new();
		let peer = test_peer_id();
		let rtt = Duration::from_millis(100);

		tracker.record_sample(peer, rtt);

		let rtt = tracker.get(&peer).expect("should have RTT data");
		assert_eq!(rtt, Duration::from_millis(100));
	}

	#[test]
	fn ewma_converges_toward_samples() {
		let tracker = RttTracker::new();
		let peer = test_peer_id();

		// Start at 100ms
		tracker.record_sample(peer, Duration::from_millis(100));

		// Feed many samples at 200ms — should converge toward 200ms
		for _ in 0..50 {
			tracker.record_sample(peer, Duration::from_millis(200));
		}

		let rtt = tracker.get(&peer).unwrap();
		// After many samples at 200ms, smoothed should be very close
		assert!(
			rtt > Duration::from_millis(195),
			"smoothed RTT should converge toward 200ms, got {rtt:?}",
		);
	}

	#[test]
	fn remove_clears_state() {
		let tracker = RttTracker::new();
		let peer = test_peer_id();

		tracker.record_sample(peer, Duration::from_millis(100));
		assert!(tracker.get(&peer).is_some());

		tracker.remove(&peer);
		assert!(tracker.get(&peer).is_none());
	}

	#[test]
	fn peer_info_rtt_below_optimistic_for_none() {
		let tracker = RttTracker::new();
		let network_id = crate::NetworkId::random();
		let secret = iroh::SecretKey::generate(&mut rand::rng());
		let address = iroh::EndpointAddr::from(secret.public());
		let entry = PeerEntry::new(network_id, address);

		let info = PeerInfo::from_tracker(&entry, &tracker);

		// No RTT data — should be optimistic
		assert!(info.rtt().is_none());
		assert!(info.rtt_below(Duration::from_millis(1)));
	}

	#[test]
	fn peer_info_rtt_below_checks_threshold() {
		let tracker = RttTracker::new();
		let network_id = crate::NetworkId::random();
		let secret = iroh::SecretKey::generate(&mut rand::rng());
		let address = iroh::EndpointAddr::from(secret.public());
		let entry = PeerEntry::new(network_id, address);

		// Seed the tracker with a 150ms RTT
		tracker.record_sample(*entry.id(), Duration::from_millis(150));

		let info = PeerInfo::from_tracker(&entry, &tracker);
		assert!(info.rtt_below(Duration::from_millis(200)));
		assert!(!info.rtt_below(Duration::from_millis(100)));
	}

	#[test]
	fn peer_info_derefs_to_peer_entry() {
		let tracker = RttTracker::new();
		let network_id = crate::NetworkId::random();
		let secret = iroh::SecretKey::generate(&mut rand::rng());
		let address = iroh::EndpointAddr::from(secret.public());
		let entry = PeerEntry::new(network_id, address).add_tags("test-tag");

		let info = PeerInfo::from_tracker(&entry, &tracker);

		// Should auto-deref to PeerEntry methods
		assert_eq!(info.tags(), entry.tags());
		assert_eq!(info.id(), entry.id());
		assert_eq!(info.network_id(), entry.network_id());
	}
}
