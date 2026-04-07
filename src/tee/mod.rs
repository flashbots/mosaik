//! Trusted Execution Environment (TEE) support.

// Support for Intel TDX (Trust Domain Extensions) TEEs.
#[cfg(feature = "tdx")]
pub mod tdx;
