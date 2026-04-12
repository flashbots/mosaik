use {
	super::{
		IntoMeasurement,
		Measurements,
		MeasurementsCriteria,
		local::Error,
		ticket::TdxTicket,
	},
	crate::{
		UniqueId,
		discovery::PeerEntry,
		tickets::{Expiration, InvalidTicket, TicketValidator},
	},
	chrono::{DateTime, Utc},
	std::time::Duration,
};

/// A TDX-based ticket validator.
///
/// This validator checks that a peer's TDX measurements (`MR_TD` and `RTMRs`)
/// match specified criteria, and that the ticket's system data matches the
/// peer's identity and network. It allows for specifying a baseline set of
/// criteria that must be satisfied, as well as additional sets of criteria that
/// can be satisfied as alternatives.
///
/// Rejects any tickets with invalid TDX signatures in their TDX Quote.
#[derive(Clone)]
pub struct Tdx {
	/// The `MeasurementsCriteria` that must be satisfied by all peers for their
	/// tickets to be considered valid.
	///
	/// This allows for specifying a baseline TDX measurement profile that all
	/// peers must satisfy, while the `any` field allows for specifying
	/// additional acceptable profiles that can be satisfied as alternatives to
	/// the baseline.
	baseline: MeasurementsCriteria,

	/// A list of `MeasurementsCriteria`, at least one of which must be satisfied
	/// by a peer for their ticket to be considered valid. This allows for
	/// specifying multiple acceptable TDX measurement profiles, any of which can
	/// be satisfied in addition to the baseline.
	any: Vec<MeasurementsCriteria>,

	/// The maximum age of a ticket's TDX Quote. A ticket whose `quoted_at`
	/// timestamp is older than `now - max_age` will be rejected. This is based
	/// on the peer's machine local clock and should be set with some tolerance
	/// for clock skew.
	max_age: Option<Duration>,

	/// The earliest acceptable `quoted_at` timestamp. A ticket whose TDX Quote
	/// was generated before this time will be rejected. Useful for invalidating
	/// all tickets generated before a known-good point in time (e.g., after a
	/// security patch or firmware update).
	not_before: Option<DateTime<Utc>>,

	/// The maximum allowed lifetime of a ticket, measured as the duration
	/// between `quoted_at` and `expiration`. When set, tickets with
	/// `Expiration::Never` are rejected (infinite lifetime exceeds any bound),
	/// and tickets whose lifetime exceeds this duration are also rejected.
	max_lifetime: Option<Duration>,
}

// Public API
impl Tdx {
	pub const CLASS: UniqueId = super::TICKET_CLASS;

	#[must_use]
	pub const fn baseline(baseline: MeasurementsCriteria) -> Self {
		Self {
			baseline,
			any: Vec::new(),
			max_age: None,
			not_before: None,
			max_lifetime: None,
		}
	}

	/// Creates a new `Tdx` that allows any ticket as long as it carries
	/// valid TDX signatures in their TDX Quote.
	#[must_use]
	pub const fn empty() -> Self {
		Self::baseline(MeasurementsCriteria::new())
	}

	/// Creates a new `Tdx` that allows any ticket as long as it carries
	/// valid TDX signatures in their TDX Quote.
	#[must_use]
	pub const fn new() -> Self {
		Self::empty()
	}

	/// Requires that all tickets satisfy the given `Measurement` for `MR_TD` in
	/// addition to the baseline criteria and other existing variants. This is a
	/// convenience method for adding a common requirement on the `MR_TD` TDX
	/// measurement.
	#[must_use]
	pub fn require_mrtd(mut self, mrtd: impl IntoMeasurement) -> Self {
		self.baseline = self.baseline.require_mrtd(mrtd.into_measurement());
		self
	}

	/// Requires that all tickets satisfy the given `Measurement` for `RTMR0` in
	/// addition to the baseline criteria and other existing variants. This is a
	/// convenience method for adding a common requirement on the `RTMR0` TDX
	/// measurement.
	#[must_use]
	pub fn require_rtmr0(mut self, rtmr0: impl IntoMeasurement) -> Self {
		self.baseline = self.baseline.require_rtmr0(rtmr0.into_measurement());
		self
	}

	/// Requires that all tickets satisfy the given `Measurement` for `RTMR1` in
	/// addition to the baseline criteria and other existing variants. This is a
	/// convenience method for adding a common requirement on the `RTMR1` TDX
	/// measurement.
	#[must_use]
	pub fn require_rtmr1(mut self, rtmr1: impl IntoMeasurement) -> Self {
		self.baseline = self.baseline.require_rtmr1(rtmr1.into_measurement());
		self
	}

	/// Requires that all tickets satisfy the given `Measurement` for `RTMR2` in
	/// addition to the baseline criteria and other existing variants. This is a
	/// convenience method for adding a common requirement on the `RTMR2` TDX
	/// measurement.
	#[must_use]
	pub fn require_rtmr2(mut self, rtmr2: impl IntoMeasurement) -> Self {
		self.baseline = self.baseline.require_rtmr2(rtmr2.into_measurement());
		self
	}

	/// Requires that all tickets satisfy the given `Measurement` for `RTMR3` in
	/// addition to the baseline criteria and other existing variants. This is a
	/// convenience method for adding a common requirement on the `RTMR3` TDX
	/// measurement.
	#[must_use]
	pub fn require_rtmr3(mut self, rtmr3: impl IntoMeasurement) -> Self {
		self.baseline = self.baseline.require_rtmr3(rtmr3.into_measurement());
		self
	}

	/// Creates a new `Tdx` that requires all peers to have the same
	/// TDX measurements (`MR_TD` and all `RTMR`s) as the local machine.
	///
	/// This reads the local TDX measurements from hardware and uses them as
	/// the baseline criteria. Returns an error if TDX is not available.
	pub fn from_local() -> Result<Self, Error> {
		let local = Measurements::local()?;
		Ok(Self::baseline(MeasurementsCriteria::from(&local)))
	}

	/// Requires that all tickets have the same `MR_TD` measurement as the
	/// local machine. Reads the local measurement from TDX hardware.
	///
	/// Returns an error if TDX is not available on this platform.
	pub fn require_own_mrtd(self) -> Result<Self, Error> {
		let local = Measurements::local()?;
		Ok(self.require_mrtd(local.mrtd()))
	}

	/// Requires that all tickets have the same `RTMR0` measurement as the
	/// local machine. Reads the local measurement from TDX hardware.
	///
	/// Returns an error if TDX is not available on this platform.
	pub fn require_own_rtmr0(self) -> Result<Self, Error> {
		let local = Measurements::local()?;
		Ok(self.require_rtmr0(local.rtmr0()))
	}

	/// Requires that all tickets have the same `RTMR1` measurement as the
	/// local machine. Reads the local measurement from TDX hardware.
	///
	/// Returns an error if TDX is not available on this platform.
	pub fn require_own_rtmr1(self) -> Result<Self, Error> {
		let local = Measurements::local()?;
		Ok(self.require_rtmr1(local.rtmr1()))
	}

	/// Requires that all tickets have the same `RTMR2` measurement as the
	/// local machine. Reads the local measurement from TDX hardware.
	///
	/// Returns an error if TDX is not available on this platform.
	pub fn require_own_rtmr2(self) -> Result<Self, Error> {
		let local = Measurements::local()?;
		Ok(self.require_rtmr2(local.rtmr2()))
	}

	/// Requires that all tickets have the same `RTMR3` measurement as the
	/// local machine. Reads the local measurement from TDX hardware.
	///
	/// Returns an error if TDX is not available on this platform.
	pub fn require_own_rtmr3(self) -> Result<Self, Error> {
		let local = Measurements::local()?;
		Ok(self.require_rtmr3(local.rtmr3()))
	}

	/// Returns a new `Tdx` that allows tickets that satisfy the given
	/// `MeasurementsCriteria` in addition to the baseline criteria and other
	/// existing variants.
	#[must_use]
	pub fn allow_variant(mut self, criteria: MeasurementsCriteria) -> Self {
		self.any.push(criteria);
		self
	}

	/// Rejects tickets whose TDX Quote is older than the given duration.
	/// A ticket is rejected if `now - quoted_at > max_age`.
	///
	/// This is based on the peer's machine local clock, so the duration
	/// should include some tolerance for clock skew between peers.
	#[must_use]
	pub const fn max_age(mut self, max_age: Duration) -> Self {
		self.max_age = Some(max_age);
		self
	}

	/// Rejects tickets whose TDX Quote was generated before the given
	/// timestamp. Useful for invalidating all tickets generated before a
	/// known-good point in time, such as after a security patch or firmware
	/// update.
	#[must_use]
	pub const fn not_before(mut self, not_before: DateTime<Utc>) -> Self {
		self.not_before = Some(not_before);
		self
	}

	/// Rejects tickets whose lifetime exceeds the given duration. The
	/// lifetime is measured as the duration between `quoted_at` and
	/// `expiration`. When set, non-expiring tickets (`Expiration::Never`)
	/// are also rejected.
	#[must_use]
	pub const fn max_lifetime(mut self, max_lifetime: Duration) -> Self {
		self.max_lifetime = Some(max_lifetime);
		self
	}
}

impl Default for Tdx {
	/// Creates a new `Tdx` that allows any ticket as long as it carries
	/// valid TDX signatures in their TDX Quote.
	fn default() -> Self {
		Self::new()
	}
}

impl TicketValidator for Tdx {
	fn class(&self) -> UniqueId {
		Self::CLASS
	}

	fn signature(&self) -> UniqueId {
		let sig = self
			.any
			.iter()
			.fold(self.class().derive(self.baseline.signature()), |s, c| {
				s.derive(c.signature())
			});

		let sig = self.max_age.map_or_else(
			|| sig.derive([0u8; 8]),
			|d| sig.derive(d.as_secs().to_le_bytes()),
		);

		let sig = self.not_before.map_or_else(
			|| sig.derive([0u8; 8]),
			|dt| sig.derive(dt.timestamp().to_le_bytes()),
		);

		self.max_lifetime.map_or_else(
			|| sig.derive([0u8; 8]),
			|d| sig.derive(d.as_secs().to_le_bytes()),
		)
	}

	fn validate(
		&self,
		bytes: &[u8],
		peer: &PeerEntry,
	) -> Result<Expiration, InvalidTicket> {
		let Ok(ticket) = TdxTicket::try_from(bytes) else {
			return Err(InvalidTicket);
		};

		let measurements = ticket.measurements();

		if !self.baseline.matches(&measurements) {
			return Err(InvalidTicket);
		}

		if !self.any.is_empty()
			&& !self.any.iter().any(|c| c.matches(&measurements))
		{
			return Err(InvalidTicket);
		}

		if ticket.peer_id() != peer.id() {
			return Err(InvalidTicket);
		}

		if ticket.network_id() != peer.network_id() {
			return Err(InvalidTicket);
		}

		if let Some(max_age) = self.max_age {
			let age = (Utc::now() - ticket.quoted_at())
				.to_std()
				.unwrap_or(Duration::MAX);
			if age > max_age {
				return Err(InvalidTicket);
			}
		}

		if let Some(not_before) = self.not_before
			&& *ticket.quoted_at() < not_before
		{
			return Err(InvalidTicket);
		}

		if let Some(max_lifetime) = self.max_lifetime {
			match ticket.expiration() {
				Expiration::Never => return Err(InvalidTicket),
				Expiration::At(expires_at) => {
					let lifetime = (*expires_at - *ticket.quoted_at())
						.to_std()
						.unwrap_or(Duration::MAX);
					if lifetime > max_lifetime {
						return Err(InvalidTicket);
					}
				}
			}
		}

		if ticket.expiration().is_expired() {
			return Err(InvalidTicket);
		}

		Ok(*ticket.expiration())
	}
}
