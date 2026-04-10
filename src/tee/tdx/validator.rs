use crate::{
	TicketValidator,
	UniqueId,
	discovery::PeerEntry,
	id,
	primitives::{Expiration, InvalidTicket},
	tee::tdx::{MeasurementsCriteria, ticket::TdxTicket},
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
pub struct TdxValidator {
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
}

// Public API
impl TdxValidator {
	pub const CLASS: UniqueId = id!("mosaik.tee.tdx.ticket-validator.v1");

	#[must_use]
	pub const fn baseline(baseline: MeasurementsCriteria) -> Self {
		Self {
			baseline,
			any: Vec::new(),
		}
	}

	/// Creates a new `TdxValidator` that allows any ticket as long as it carries
	/// valid TDX signatures in their TDX Quote.
	#[must_use]
	pub const fn empty() -> Self {
		Self::baseline(MeasurementsCriteria::new())
	}

	/// Creates a new `TdxValidator` that allows any ticket as long as it carries
	/// valid TDX signatures in their TDX Quote.
	#[must_use]
	pub const fn new() -> Self {
		Self::empty()
	}

	/// Returns a new `TdxValidator` that allows tickets that satisfy the given
	/// `MeasurementsCriteria` in addition to the baseline criteria and other
	/// existing variants.
	#[must_use]
	pub fn allow_variant(mut self, criteria: MeasurementsCriteria) -> Self {
		self.any.push(criteria);
		self
	}
}

impl Default for TdxValidator {
	fn default() -> Self {
		Self::baseline(MeasurementsCriteria::new())
	}
}

impl TicketValidator for TdxValidator {
	fn class(&self) -> UniqueId {
		Self::CLASS
	}

	fn signature(&self) -> UniqueId {
		self
			.any
			.iter()
			.fold(self.class().derive(self.baseline.signature()), |s, c| {
				s.derive(c.signature())
			})
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

		Ok(*ticket.expiration())
	}
}
