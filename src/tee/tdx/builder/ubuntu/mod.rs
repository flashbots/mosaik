//! Ubuntu-based TDX image builder.
//!
//! Produces a bootable initramfs containing the calling crate's
//! binary as `/init`, suitable for direct-boot TDX guests
//! (`qemu -kernel ... -initrd ...`).
//!
//! # Usage
//!
//! In your crate's `build.rs`:
//!
//! ```rust,ignore
//! fn main() {
//!     mosaik::tee::tdx::build::ubuntu().build();
//! }
//! ```
//!
//! # What it does
//!
//! 1. Cross-compiles the crate for `x86_64-unknown-linux-gnu`.
//! 2. Downloads Ubuntu minimal rootfs (cached across builds).
//! 3. Assembles a CPIO archive from the rootfs with the binary and kernel
//!    modules overlaid.
//! 4. gzip-compresses the CPIO into
//!    `target/{profile}/<crate>-initramfs.cpio.gz`.
//! 5. Downloads or uses a provided TDX-capable kernel + OVMF firmware.
//! 6. Generates a `launch-tdx.sh` helper and a self-extracting `run-qemu.sh`.
//! 7. Precomputes the MRTD from ovmf.fd for attestation.
//!
//! # Customisation
//!
//! All builder methods are optional — defaults match an
//! auto-downloaded Ubuntu 24.04 kernel with Ubuntu 24.04.2
//! minimal rootfs.

mod builder;
mod cross;

pub use builder::UbuntuBuilder;
