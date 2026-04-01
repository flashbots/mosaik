//! TEE (Trusted Execution Environment) integration for Mosaik.
//!
//! This module provides SGX enclave attestation support via the Fortanix
//! [`sgx-isa`](https://crates.io/crates/sgx-isa) crate and Gramine's
//! `/dev/attestation/` pseudo-filesystem interface.
//!
//! # How it works
//!
//! When running inside a Gramine SGX enclave, each node can read its own
//! **MRENCLAVE** — a 32-byte hash of the enclave's code and initial data that
//! is unique to a specific binary build. This measurement is published as a
//! discovery [`Ticket`] so that peers can verify a node is genuinely running
//! inside a trusted enclave image.
//!
//! # Measurement reading
//!
//! Gramine exposes SGX attestation via a virtual filesystem:
//!
//! | Path | Direction | Description |
//! |------|-----------|-------------|
//! | `/dev/attestation/user_report_data` | write | 64 bytes of custom data (e.g. peer key) |
//! | `/dev/attestation/report` | read | Raw 432-byte `sgx_isa::Report` |
//! | `/dev/attestation/quote` | read | DCAP quote for remote attestation |
//!
//! Reading the report in `gramine-direct` (simulation) returns all-zero
//! measurements. On real SGX hardware (`gramine-sgx`) the values are genuine.
//!
//! # Usage
//!
//! ```no_run
//! use mosaik::tee::{SgxValidator, make_sgx_ticket, read_measurement};
//!
//! # async fn example(network: &mosaik::Network) -> anyhow::Result<()> {
//! // 1. Read this enclave's MRENCLAVE at startup.
//! let measurement = read_measurement()?;
//! println!("MRENCLAVE: {measurement}");
//!
//! // 2. Publish it so that other peers can verify this node's attestation.
//! network
//! 	.discovery()
//! 	.add_ticket(make_sgx_ticket(&measurement));
//!
//! // 3. Gate subscriptions: only accept peers from the same enclave image.
//! let _producer = network
//! 	.streams()
//! 	.producer::<String>()
//! 	.with_ticket_validator(SgxValidator::trusted_measurements([measurement]))
//! 	.build()?;
//! # Ok(())
//! # }
//! ```

mod validator;

pub use validator::SgxValidator;
use {
	crate::{Ticket, id},
	serde::{Deserialize, Serialize},
	std::path::Path,
};

/// The MRENCLAVE measurement of an SGX enclave: a 32-byte hash of the
/// enclave's code and initial data as loaded by the SGX hardware.
///
/// All nodes built from the same source binary will produce the same
/// MRENCLAVE. In Gramine simulation mode (`gramine-direct`) the value is
/// all-zeros; on real hardware it reflects the genuine enclave measurement.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Measurement(pub(crate) [u8; 32]);

impl Measurement {
	/// Returns a reference to the raw 32 measurement bytes.
	pub const fn as_bytes(&self) -> &[u8; 32] {
		&self.0
	}

	/// Constructs a `Measurement` from raw bytes.
	pub const fn from_bytes(bytes: [u8; 32]) -> Self {
		Self(bytes)
	}
}

impl core::fmt::Debug for Measurement {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		write!(f, "Measurement({})", hex::encode(self.0))
	}
}

impl core::fmt::Display for Measurement {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		write!(f, "{}", hex::encode(self.0))
	}
}

/// Errors that can occur when reading the enclave measurement.
#[derive(Debug, thiserror::Error)]
pub enum MeasurementError {
	/// The process is not running inside a Gramine SGX enclave.
	///
	/// `/dev/attestation/` is absent, which means either the binary is running
	/// natively or inside a container without Gramine.
	#[error(
		"not running inside an SGX enclave (Gramine /dev/attestation/ not found)"
	)]
	NotInEnclave,

	/// An I/O error occurred while reading or writing the attestation files.
	#[error("attestation I/O error: {0}")]
	Io(#[from] std::io::Error),

	/// The attestation report had an unexpected byte length.
	///
	/// An `sgx_isa::Report` is exactly 432 bytes; any other length indicates a
	/// misconfigured Gramine environment or an unsupported SGX ABI version.
	#[error(
		"attestation report has unexpected size (expected {expected} bytes, got \
		 {got})"
	)]
	InvalidReport { expected: usize, got: usize },
}

/// Path to Gramine's attestation report pseudo-file.
const GRAMINE_REPORT_PATH: &str = "/dev/attestation/report";

/// Path to Gramine's user-data input pseudo-file (written before reading).
const GRAMINE_USER_DATA_PATH: &str = "/dev/attestation/user_report_data";

/// Read the current enclave's MRENCLAVE measurement from Gramine's
/// `/dev/attestation/report` pseudo-file.
///
/// Returns [`MeasurementError::NotInEnclave`] when the attestation FS is
/// absent, i.e. the binary is not running inside a Gramine SGX environment.
///
/// # Simulation mode
///
/// In `gramine-direct` the measurement bytes are all-zeros.  The mechanism
/// is end-to-end testable, but no real SGX guarantee is provided until the
/// binary is deployed with `gramine-sgx`.
pub fn read_measurement() -> Result<Measurement, MeasurementError> {
	if !Path::new(GRAMINE_REPORT_PATH).exists() {
		return Err(MeasurementError::NotInEnclave);
	}
	let bytes = std::fs::read(GRAMINE_REPORT_PATH)?;
	parse_mrenclave(&bytes)
}

/// Read the current enclave's MRENCLAVE, binding `user_data` into the report.
///
/// Writing user data (e.g. the peer's public key) before reading the report
/// makes the attestation specific to that key, so a valid ticket cannot be
/// replayed for a different peer identity.
///
/// The `user_data` slice must be exactly 64 bytes (padding with zeros is fine).
pub fn read_measurement_for(
	user_data: &[u8; 64],
) -> Result<Measurement, MeasurementError> {
	if !Path::new(GRAMINE_REPORT_PATH).exists() {
		return Err(MeasurementError::NotInEnclave);
	}
	std::fs::write(GRAMINE_USER_DATA_PATH, user_data)?;
	let bytes = std::fs::read(GRAMINE_REPORT_PATH)?;
	parse_mrenclave(&bytes)
}

/// Parse the MRENCLAVE field from a raw SGX `Report` byte slice.
///
/// The `sgx_isa::Report` struct is 432 bytes. MRENCLAVE sits at the offset
/// defined by the SGX ABI (Intel SDM Vol. 3D, Table 38-21). We use the
/// `sgx_isa` crate's `try_copy_from` to deserialise the struct and then
/// read the `mrenclave` field directly, rather than relying on manual offsets.
fn parse_mrenclave(bytes: &[u8]) -> Result<Measurement, MeasurementError> {
	let report = sgx_isa::Report::try_copy_from(bytes).ok_or(
		MeasurementError::InvalidReport {
			expected: core::mem::size_of::<sgx_isa::Report>(),
			got: bytes.len(),
		},
	)?;
	Ok(Measurement(report.mrenclave))
}

/// Build a discovery [`Ticket`] containing the MRENCLAVE measurement.
///
/// Add this ticket to the local peer's discovery entry so that other peers
/// running an [`SgxValidator`] can verify the attestation:
///
/// ```no_run
/// use mosaik::tee::{make_sgx_ticket, read_measurement};
///
/// # async fn example(network: &mosaik::Network) -> anyhow::Result<()> {
/// let measurement = read_measurement()?;
/// network
/// 	.discovery()
/// 	.add_ticket(make_sgx_ticket(&measurement));
/// # Ok(())
/// # }
/// ```
pub fn make_sgx_ticket(measurement: &Measurement) -> Ticket {
	Ticket::new(
		id!("mosaik.tee.sgx_mrenclave_ticket"),
		bytes::Bytes::copy_from_slice(measurement.as_bytes()),
	)
}
