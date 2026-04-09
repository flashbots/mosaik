use {
	super::{Measurement, Measurements, ticket::ExtraData},
	crate::{
		Network,
		Ticket,
		primitives::{Expiration, sealed::Sealed},
		tee::tdx::ticket::TdxTicketData,
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
	TicketError(#[from] super::ticket::TicketError),

	#[error("TDX expiration time is in the past")]
	ExpirationInThePast,
}

pub struct TdxNetworkHelpers<'a>(&'a Network);

impl TdxNetworkHelpers<'_> {
	/// Creates a TDX attestation ticket for the given network and measurement
	/// that never expires.
	pub fn ticket(&self) -> Result<Ticket, Error> {
		self.ticket_with_expiration(Expiration::Never)
	}

	/// Creates a TDX ticket for the given network with the specified expiration.
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
		Ok(TdxTicketData::new(quote_bytes, extra_data)?.try_into()?)
	}

	/// Returns all measurements (`MR_TD` and `RTMR`s) of the local machine's TDX
	/// environment.
	///
	/// Note: This requires generating a TDX quote and parsing it, so it may be
	/// slow. If you're going to request several measurements, it's more efficient
	/// to call `ticket()` once convert it to `TdxTicketData` and extract the
	/// measurements from there.
	pub fn measurements(&self) -> Result<Measurements, Error> {
		let ticket: TdxTicketData = self.ticket()?.try_into()?;
		Ok(Measurements::from_quote(ticket.quote()))
	}

	/// Returns the `MR_TD` measurement of the local machine's TDX environment.
	///
	/// Note: This requires generating a TDX quote and parsing it, so it may be
	/// slow. If you're going to request several measurements, it's more efficient
	/// to call `measurements()` once and extract the individual measurements from
	/// the returned `Measurements` struct.
	pub fn mrtd(&self) -> Result<Measurement, Error> {
		self.measurements().map(|m| m.mrtd)
	}

	/// Returns the `RTMR[0]` measurement of the local machine's TDX environment.
	///
	/// Note: This requires generating a TDX quote and parsing it, so it may be
	/// slow. If you're going to request several measurements, it's more efficient
	/// to call `measurements()` once and extract the individual measurements from
	/// the returned `Measurements` struct.
	pub fn rtmr0(&self) -> Result<Measurement, Error> {
		self.measurements().map(|m| m.rtmr[0])
	}

	/// Returns the `RTMR[1]` measurement of the local machine's TDX environment.
	///
	/// Note: This requires generating a TDX quote and parsing it, so it may be
	/// slow. If you're going to request several measurements, it's more efficient
	/// to call `measurements()` once and extract the individual measurements from
	/// the returned `Measurements` struct.
	pub fn rtmr1(&self) -> Result<Measurement, Error> {
		self.measurements().map(|m| m.rtmr[1])
	}

	/// Returns the `RTMR[2]` measurement of the local machine's TDX environment.
	///
	/// Note: This requires generating a TDX quote and parsing it, so it may be
	/// slow. If you're going to request several measurements, it's more efficient
	/// to call `measurements()` once and extract the individual measurements from
	/// the returned `Measurements` struct.
	pub fn rtmr2(&self) -> Result<Measurement, Error> {
		self.measurements().map(|m| m.rtmr[2])
	}

	/// Returns the `RTMR[3]` measurement of the local machine's TDX environment.
	///
	/// Note: This requires generating a TDX quote and parsing it, so it may be
	/// slow. If you're going to request several measurements, it's more efficient
	/// to call `measurements()` once and extract the individual measurements from
	/// the returned `Measurements` struct.
	pub fn rtmr3(&self) -> Result<Measurement, Error> {
		self.measurements().map(|m| m.rtmr[3])
	}
}

/// Extension trait for `Network` to provide TDX-specific ticket support.
pub trait NetworkTicketExt: Sealed {
	fn tdx(&self) -> TdxNetworkHelpers<'_>;
}

impl NetworkTicketExt for Network {
	fn tdx(&self) -> TdxNetworkHelpers<'_> {
		TdxNetworkHelpers(self)
	}
}
