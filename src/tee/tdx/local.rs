use {
	super::{Measurement, Measurements, ticket::ExtraData},
	crate::{
		Network,
		primitives::sealed::Sealed,
		tee::tdx::ticket::TdxTicket,
		tickets::{Expiration, Ticket},
	},
	chrono::Utc,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("TDX is not supported on this platform")]
	TdxUnsupported,

	#[error("Failed to generate TDX quote: {0}")]
	QuoteGenerationError(#[from] configfs_tsm::QuoteGenerationError),

	#[error("Failed to parse TDX quote: {0}")]
	QuoteParseError(#[from] tdx_quote::QuoteParseError),

	#[error("Invalid TDX ticket: {0}")]
	TicketError(#[from] super::ticket::TdxTicketError),

	#[error("TDX expiration time is in the past")]
	ExpirationInThePast,
}

impl Measurements {
	/// Reads the local machine's TDX measurements (`MR_TD` and `RTMR`s) from
	/// the TDX hardware.
	///
	/// This is a standalone function that does not require a `Network` instance.
	/// It generates a TDX quote and extracts the measurements from it.
	///
	/// Returns an error if TDX is not available on this platform.
	pub fn local() -> Result<Self, Error> {
		let quote = configfs_tsm::create_tdx_quote([0u8; 64])?;
		let quote = tdx_quote::Quote::from_bytes(&quote)?;
		Ok(Self::from_quote(&quote))
	}
}

/// Provides TDX-specific operations for a `Network`, such as creating TDX
/// tickets and retrieving TDX measurements.
pub struct NetworkTdxOps<'a>(&'a Network);

impl NetworkTdxOps<'_> {
	/// Checks if TDX is supported and available for generating tickets and
	/// retrieving measurements on the local machine.
	#[allow(clippy::unused_self)]
	pub fn available(&self) -> bool {
		Measurements::local().is_ok()
	}

	/// Creates a TDX attestation ticket for the current network instance and
	/// measurement that never expires.
	pub fn ticket(&self) -> Result<Ticket, Error> {
		self.ticket_with_expiration(Expiration::Never)
	}

	/// Creates a TDX ticket for the current network instance with the specified
	/// expiration.
	pub fn ticket_with_expiration(
		&self,
		expiration: Expiration,
	) -> Result<Ticket, Error> {
		if expiration.is_expired() {
			return Err(Error::ExpirationInThePast);
		}

		let extra_data = ExtraData {
			peer_id: self.0.local().id(),
			network_id: *self.0.local().network_id(),
			started_at: self.0.discovery().me().started_at(),
			quoted_at: Utc::now(),
			expiration,
		};

		let report_data: [u8; 64] = {
			let mut data = [0u8; 64];
			data[..32].copy_from_slice(extra_data.signature().as_bytes());
			data
		};

		let quote_bytes = configfs_tsm::create_tdx_quote(report_data)?;
		Ok(TdxTicket::new(quote_bytes, extra_data)?.try_into()?)
	}

	/// Generates a TDX ticket for the local machine and adds it to the network's
	/// discovery info, making it available for other peers to see and verify.
	///
	/// This is a convenience method that combines `ticket()` and
	/// `discovery().add_ticket()`. Removes any existing tickets of the same class
	/// before adding the new one to ensure that only the latest TDX ticket is
	/// present in the discovery info.
	pub fn install_own_ticket(&self) -> Result<(), Error> {
		let ticket = self.ticket()?;
		self.0.discovery().remove_tickets_of(ticket.class);
		self.0.discovery().add_ticket(ticket);
		Ok(())
	}

	/// Returns all measurements (`MR_TD` and `RTMR`s) of the local machine's TDX
	/// environment.
	///
	/// Note: This requires generating a TDX quote and parsing it, so it may be
	/// slow. If you're going to request several measurements, it's more efficient
	/// to call `ticket()` once convert it to `TdxTicket` and extract the
	/// measurements from there.
	#[allow(clippy::unused_self)]
	pub fn measurements(&self) -> Result<Measurements, Error> {
		Measurements::local()
	}

	/// Returns the `MR_TD` measurement of the local machine's TDX environment.
	///
	/// Note: This requires generating a TDX quote and parsing it, so it may be
	/// slow. If you're going to request several measurements, it's more efficient
	/// to call `measurements()` once and extract the individual measurements from
	/// the returned `Measurements` struct.
	pub fn mrtd(&self) -> Result<Measurement, Error> {
		self.measurements().map(|m| m.mrtd())
	}

	/// Returns the `RTMR[0]` measurement of the local machine's TDX environment.
	///
	/// Note: This requires generating a TDX quote and parsing it, so it may be
	/// slow. If you're going to request several measurements, it's more efficient
	/// to call `measurements()` once and extract the individual measurements from
	/// the returned `Measurements` struct.
	pub fn rtmr0(&self) -> Result<Measurement, Error> {
		self.measurements().map(|m| m.rtmr0())
	}

	/// Returns the `RTMR[1]` measurement of the local machine's TDX environment.
	///
	/// Note: This requires generating a TDX quote and parsing it, so it may be
	/// slow. If you're going to request several measurements, it's more efficient
	/// to call `measurements()` once and extract the individual measurements from
	/// the returned `Measurements` struct.
	pub fn rtmr1(&self) -> Result<Measurement, Error> {
		self.measurements().map(|m| m.rtmr1())
	}

	/// Returns the `RTMR[2]` measurement of the local machine's TDX environment.
	///
	/// Note: This requires generating a TDX quote and parsing it, so it may be
	/// slow. If you're going to request several measurements, it's more efficient
	/// to call `measurements()` once and extract the individual measurements from
	/// the returned `Measurements` struct.
	pub fn rtmr2(&self) -> Result<Measurement, Error> {
		self.measurements().map(|m| m.rtmr2())
	}

	/// Returns the `RTMR[3]` measurement of the local machine's TDX environment.
	///
	/// Note: This requires generating a TDX quote and parsing it, so it may be
	/// slow. If you're going to request several measurements, it's more efficient
	/// to call `measurements()` once and extract the individual measurements from
	/// the returned `Measurements` struct.
	pub fn rtmr3(&self) -> Result<Measurement, Error> {
		self.measurements().map(|m| m.rtmr3())
	}
}

/// Extension trait for `Network` to provide TDX-specific support.
pub trait NetworkTdxExt: Sealed {
	fn tdx(&self) -> NetworkTdxOps<'_>;
}

impl NetworkTdxExt for Network {
	fn tdx(&self) -> NetworkTdxOps<'_> {
		NetworkTdxOps(self)
	}
}
