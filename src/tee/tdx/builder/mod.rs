//! TDX image builders

use {super::Measurement, std::path::PathBuf};

mod common;
mod cpio;
mod helpers;
mod kernel;
mod mrtd;
mod ovmf;
mod scripts;

#[cfg(feature = "tdx-builder-alpine")]
pub mod alpine;

#[cfg(feature = "tdx-builder-ubuntu")]
pub mod ubuntu;

/// Output of a TDX image build — paths to generated artifacts and
/// precomputed measurements.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct BuilderOutput {
	pub initramfs_path: PathBuf,
	pub kernel_path: PathBuf,
	pub ovmf_path: PathBuf,
	pub bundle_path: PathBuf,
	pub mrtd: Measurement,
	pub rtmr1: Measurement,
	pub rtmr2: Measurement,
}
