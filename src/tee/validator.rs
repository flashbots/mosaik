//! SGX MRENCLAVE ticket validator for peer attestation.

use {
	super::Measurement,
	crate::{
		Expiration,
		InvalidTicket,
		TicketValidator,
		UniqueId,
		discovery::PeerEntry,
		id,
	},
};

/// A [`TicketValidator`] that verifies SGX MRENCLAVE attestation tickets.
///
/// Validates that a peer's discovery [`Ticket`] contains an MRENCLAVE that
/// matches one of the configured trusted measurements.  Peers that do not
/// present a valid SGX ticket — or whose MRENCLAVE is not in the trusted set
/// — are rejected.
///
/// # Example
///
/// ```no_run
/// use mosaik::tee::{SgxValidator, make_sgx_ticket, read_measurement};
///
/// # async fn example(network: &mosaik::Network) -> anyhow::Result<()> {
/// let measurement = read_measurement()?;
///
/// // Only accept subscribers running the same enclave image.
/// let _producer = network
/// 	.streams()
/// 	.producer::<String>()
/// 	.with_ticket_validator(SgxValidator::trusted_measurements([measurement]))
/// 	.build()?;
///
/// // Only subscribe to producers running the same enclave image.
/// let _consumer = network
/// 	.streams()
/// 	.consumer::<String>()
/// 	.with_ticket_validator(SgxValidator::trusted_measurements([measurement]))
/// 	.build();
/// # Ok(())
/// # }
/// ```
///
/// # Rolling upgrades
///
/// To allow two enclave versions to interoperate during a rollout, pass both
/// old and new measurements:
///
/// ```no_run
/// # use mosaik::tee::{Measurement, SgxValidator};
/// # let old_mrenclave = Measurement::from_bytes([0u8; 32]);
/// # let new_mrenclave = Measurement::from_bytes([1u8; 32]);
/// let validator =
/// 	SgxValidator::trusted_measurements([old_mrenclave, new_mrenclave]);
/// ```
///
/// # Security note
///
/// This validator checks that the MRENCLAVE bytes in the ticket match the
/// trusted set, but it does **not** verify a cryptographic quote.  A malicious
/// peer can forge any MRENCLAVE value in its ticket.  For production use,
/// replace the raw MRENCLAVE ticket with a DCAP quote and verify it against
/// Intel's PCK certificate chain.
pub struct SgxValidator {
	trusted: Vec<Measurement>,
}

impl SgxValidator {
	/// The [`UniqueId`] class of all SGX MRENCLAVE attestation tickets.
	///
	/// Use this constant to filter tickets by class when inspecting a peer:
	///
	/// ```no_run
	/// # use mosaik::tee::SgxValidator;
	/// # fn check(peer: &mosaik::discovery::PeerEntry) {
	/// let sgx_tickets = peer.tickets_of(SgxValidator::CLASS);
	/// # }
	/// ```
	///
	/// This is also the `class` field of tickets produced by [`make_sgx_ticket`].
	///
	/// [`make_sgx_ticket`]: crate::tee::make_sgx_ticket
	pub const CLASS: UniqueId = id!("mosaik.tee.sgx_mrenclave_ticket");

	/// Create a validator that accepts peers whose MRENCLAVE is in
	/// `trusted_measurements`.
	///
	/// Pass a single element `[measurement]` when all nodes run the same binary
	/// (the typical case).  Pass multiple measurements to allow rolling upgrades
	/// where different enclave versions must interoperate during a deployment.
	pub fn trusted_measurements(
		trusted_measurements: impl IntoIterator<Item = Measurement>,
	) -> Self {
		Self {
			trusted: trusted_measurements.into_iter().collect(),
		}
	}
}

impl TicketValidator for SgxValidator {
	fn class(&self) -> UniqueId {
		Self::CLASS
	}

	/// A deterministic fingerprint of this validator and its trusted
	/// measurement set.
	///
	/// Two `SgxValidator` instances with identical trusted sets will produce
	/// the same `signature()`.  This is required for correct Mosaik group ID
	/// derivation — all group members must use the same validator configuration,
	/// otherwise they will compute different group IDs and fail to bond.
	fn signature(&self) -> UniqueId {
		self
			.trusted
			.iter()
			.fold(Self::CLASS, |acc, m| acc.derive(m.as_bytes()))
	}

	fn validate(
		&self,
		ticket: &[u8],
		_peer: &PeerEntry,
	) -> Result<Expiration, InvalidTicket> {
		let bytes: [u8; 32] = ticket.try_into().map_err(|_| InvalidTicket)?;
		let m = Measurement(bytes);
		if self.trusted.contains(&m) {
			// MRENCLAVE measurements do not have an inherent expiry — a peer
			// remains trusted as long as it runs the same enclave binary.
			Ok(Expiration::Never)
		} else {
			Err(InvalidTicket)
		}
	}
}
