use {
	super::ticket::Measurement,
	crate::{Network, Ticket, primitives::sealed::Sealed},
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("TDX is not supported on this platform")]
	TdxUnsupported,

	#[error("Failed to generate TDX quote: {0}")]
	QuoteGenerationError(#[from] configfs_tsm::QuoteGenerationError),

	#[error("Failed to parse TDX quote: {0}")]
	QuoteParseError(#[from] tdx_quote::QuoteParseError),
}

pub struct TdxNetworkHelpers<'a>(&'a Network);

impl TdxNetworkHelpers<'_> {
	/// Creates a TDX attestation ticket for the given network and measurement.
	pub fn ticket(&self) -> Result<Ticket, Error> {
		let _ = self.0;
		todo!()
	}

	/// Returns the `MR_TD` measurement of the local machine's TDX environment.
	pub fn mrtd(&self) -> Result<Measurement, Error> {
		let _ = self.0;
		let quote = configfs_tsm::create_tdx_quote([0u8; 64])?;
		let quote = tdx_quote::Quote::from_bytes(quote.as_slice())?;
		Ok(quote.mrtd().into())
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
