//! Alpine-based TDX image builder.
//!
//! Produces a bootable initramfs containing the calling crate's binary as
//! `/init`, suitable for direct-boot TDX guests
//! (`qemu -kernel ... -initrd ...`).
//!
//! # Usage
//!
//! In your crate's `build.rs`:
//!
//! ```rust,ignore
//! fn main() {
//!     mosaik::tee::tdx::build::alpine().build();
//! }
//! ```
//!
//! # What it does
//!
//! 1. Cross-compiles the crate for `x86_64-unknown-linux-musl`.
//! 2. Downloads Alpine Linux minirootfs (cached across builds).
//! 3. Assembles a CPIO "newc" archive with init, busybox, the binary, and
//!    kernel modules.
//! 4. gzip-compresses the CPIO into
//!    `target/{profile}/<crate>-initramfs.cpio.gz`.
//! 5. Downloads or uses a provided TDX-capable kernel + OVMF firmware.
//! 6. Generates a `launch-tdx.sh` helper and a self-extracting `run-qemu.sh`.
//! 7. Precomputes the MRTD from ovmf.fd for attestation.
//!
//! # Customisation
//!
//! All builder methods are optional — defaults match an auto-downloaded
//! Ubuntu 24.04 kernel with Alpine 3.21.0 minirootfs.

mod builder;
mod cpio;
mod cross;
mod helpers;
mod kernel;
mod mrtd;
mod ovmf;
mod scripts;

pub use builder::{AlpineBuilder, AlpineBuilderOutput};
