use crate::{
	TicketValidator,
	UniqueId,
	discovery::PeerEntry,
	id,
	primitives::{Expiration, InvalidTicket, encoding},
	tee::tdx::{MeasurementsCriteria, ticket::TdxTicket},
};

/// A TDX-based ticket validator.
#[derive(Clone)]
pub struct TdxValidator {
	all: MeasurementsCriteria,
	any: Vec<MeasurementsCriteria>,
}

// Public API
impl TdxValidator {
	pub const CLASS: UniqueId = id!("mosaik.tickets.tdx.v1");

	/// Creates a new validator that accepts any TDX quote with a valid
	/// Intel certificate chain and correct peer identity binding.
	///
	/// Use [`require_mrtd`] and [`require_rtmr`] to add measurement
	/// constraints.
	///
	/// [`require_mrtd`]: TdxValidator::require_mrtd
	/// [`require_rtmr`]: TdxValidator::require_rtmr
	#[must_use]
	pub const fn new(all: MeasurementsCriteria) -> Self {
		Self {
			all,
			any: Vec::new(),
		}
	}
}

impl Default for TdxValidator {
	fn default() -> Self {
		Self::new(MeasurementsCriteria::new())
	}
}

impl TicketValidator for TdxValidator {
	fn class(&self) -> UniqueId {
		Self::CLASS
	}

	fn signature(&self) -> UniqueId {
		let mut sig = self.class();

		let zero = [0u8; 48];

		sig = sig.derive(self.all.mrtd.as_ref().map_or(&zero, |m| m.as_bytes()));
		sig = sig.derive(self.all.rtmr[0].as_ref().map_or(&zero, |m| m.as_bytes()));
		sig = sig.derive(self.all.rtmr[1].as_ref().map_or(&zero, |m| m.as_bytes()));
		sig = sig.derive(self.all.rtmr[2].as_ref().map_or(&zero, |m| m.as_bytes()));
		sig = sig.derive(self.all.rtmr[3].as_ref().map_or(&zero, |m| m.as_bytes()));

		for (ix, r) in self.any.iter().enumerate() {
			sig = sig.derive((ix as u64).to_le_bytes());
			sig = sig.derive(r.mrtd.as_ref().map_or(&zero, |m| m.as_bytes()));
			sig = sig.derive(r.rtmr[0].as_ref().map_or(&zero, |m| m.as_bytes()));
			sig = sig.derive(r.rtmr[1].as_ref().map_or(&zero, |m| m.as_bytes()));
			sig = sig.derive(r.rtmr[2].as_ref().map_or(&zero, |m| m.as_bytes()));
			sig = sig.derive(r.rtmr[3].as_ref().map_or(&zero, |m| m.as_bytes()));
		}

		sig
	}

	fn validate(
		&self,
		ticket_bytes: &[u8],
		peer: &PeerEntry,
	) -> Result<Expiration, InvalidTicket> {
		let Ok(ticket) = encoding::deserialize::<TdxTicket>(ticket_bytes) else {
			return Err(InvalidTicket);
		};

		let measurements = ticket.measurements();

		if !self.all.matches(&measurements) {
			return Err(InvalidTicket);
		}

		if !self.any.is_empty()
			&& !self.any.iter().any(|c| c.matches(&measurements))
		{
			return Err(InvalidTicket);
		}

		if ticket.system().peer_id != *peer.id() {
			return Err(InvalidTicket);
		}

		if ticket.system().network_id != peer.network_id() {
			return Err(InvalidTicket);
		}

		Ok(ticket.system().expires_at)
	}
}
