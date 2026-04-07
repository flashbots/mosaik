use {
	crate::{
		TicketValidator,
		UniqueId,
		discovery::PeerEntry,
		id,
		primitives::{Expiration, InvalidTicket, encoding},
		tee::tdx::ticket::{Measurement, TdxTicket},
	},
	tdx_quote::Quote,
};

/// A TDX-based ticket validator.
///
/// Validates tickets that are raw Intel TDX quotes (v4 or v5). The quote is
/// verified against Intel's root CA certificate chain and the configured
/// measurement registers.
///
/// # Peer identity binding
///
/// The quote's `REPORTDATA` field must equal `SHA-512(peer_public_key)`. This
/// cryptographically binds the attestation to the specific mosaik peer: a
/// different peer cannot reuse the same quote.
///
/// # Ticket generation
///
/// Tickets are generated on TDX-capable hardware using
/// [`generate_tdx_ticket`], which writes the peer's public key hash into the
/// quote's `REPORTDATA` field via the Linux `configfs-tsm` interface.
///
/// # Group identity
///
/// [`TicketValidator::signature`] is derived from the configured measurements,
/// so two [`TdxQuoteValidator`] instances with different measurement
/// expectations will produce different signatures — and therefore different
/// group ids.
#[derive(Clone)]
pub struct TdxValidator {
	/// Expected MRTD (hash of the initial TD image). If `None`, any MRTD is
	/// accepted.
	mrtd: Option<Measurement>,

	/// Expected RTMR values per register index (0–3). `None` means the
	/// corresponding register is not checked.
	rtmr: [Option<Measurement>; 4],
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
	pub const fn new() -> Self {
		Self {
			mrtd: None,
			rtmr: [None, None, None, None],
		}
	}

	/// Requires the quote's MRTD to match the given value.
	///
	/// MRTD is the SHA-384 hash of the initial TD image (firmware + kernel
	/// loaded at boot). Setting this pins the accepted workload to a specific
	/// software image.
	#[must_use]
	pub const fn require_mrtd(mut self, mrtd: Measurement) -> Self {
		self.mrtd = Some(mrtd);
		self
	}

	/// Requires the quote's RTMR at the given index to match the given value.
	///
	/// Valid indices are 0–3:
	/// - RTMR0: extended by firmware
	/// - RTMR1: extended by the bootloader
	/// - RTMR2: extended by the OS
	/// - RTMR3: user-defined
	///
	/// # Panics
	///
	/// Panics if `index > 3`.
	#[must_use]
	pub fn require_rtmr(mut self, index: usize, rtmr: Measurement) -> Self {
		assert!(index < 4, "RTMR index must be 0–3, got {index}");
		self.rtmr[index] = Some(rtmr);
		self
	}
}

impl Default for TdxValidator {
	fn default() -> Self {
		Self::new()
	}
}

impl TicketValidator for TdxValidator {
	fn class(&self) -> UniqueId {
		Self::CLASS
	}

	fn signature(&self) -> UniqueId {
		let mut sig = self.class();

		if let Some(m) = &self.mrtd {
			sig = sig.derive(m.as_bytes());
		}

		for (i, r) in self.rtmr.iter().enumerate() {
			if let Some(m) = r {
				sig = sig.derive([i as u8]).derive(m.as_bytes());
			}
		}

		sig
	}

	fn validate(
		&self,
		ticket: &[u8],
		peer: &PeerEntry,
	) -> Result<Expiration, InvalidTicket> {
		let Ok(ticket) = encoding::deserialize::<TdxTicket>(ticket) else {
			return Err(InvalidTicket);
		};

		// make sure that the quote is valid and is signed by Intel's TDX root CA
		let quote = Quote::from_bytes(&ticket.quote).map_err(|_| InvalidTicket)?;
		quote.verify().map_err(|_| InvalidTicket)?;

		if !self.check_quote_measurements(&quote) {
			return Err(InvalidTicket);
		}

		let user_data_hash: [u8; 32] = quote.report_input_data()[..32]
			.try_into()
			.map_err(|_| InvalidTicket)?;

		let system_data_hash: [u8; 32] = quote.report_input_data()[32..]
			.try_into()
			.map_err(|_| InvalidTicket)?;

		Ok(ticket.system.expires_at)
	}
}

// internal api
impl TdxValidator {
	fn check_quote_measurements(&self, quote: &Quote) -> bool {
		if let Some(expected) = &self.mrtd
			&& quote.mrtd() != *expected.as_bytes()
		{
			return false;
		}

		if let Some(expected) = &self.rtmr[0]
			&& quote.rtmr0() != *expected.as_bytes()
		{
			return false;
		}

		if let Some(expected) = &self.rtmr[1]
			&& quote.rtmr1() != *expected.as_bytes()
		{
			return false;
		}

		if let Some(expected) = &self.rtmr[2]
			&& quote.rtmr2() != *expected.as_bytes()
		{
			return false;
		}

		if let Some(expected) = &self.rtmr[3]
			&& quote.rtmr3() != *expected.as_bytes()
		{
			return false;
		}

		true
	}
}
