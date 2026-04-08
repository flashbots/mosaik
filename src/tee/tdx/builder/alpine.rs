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
//!     mosaik::tee::tdx::builder::ImageBuilder::alpine().build();
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
//! 7. Precomputes the MRTD from OVMF.fd for attestation.
//!
//! # Customisation
//!
//! All builder methods are optional — defaults match an auto-downloaded
//! Ubuntu 24.04 kernel with Alpine 3.21.0 minirootfs.

use std::{
	env,
	fs::{self, File},
	io::{self, BufWriter, Write},
	path::{Path, PathBuf},
	process::Command,
};

/// Builder for Alpine-based TDX guest images.
///
/// Constructed via [`super::ImageBuilder::alpine()`].
pub struct AlpineBuilder {
	alpine_major: Option<String>,
	alpine_minor: Option<String>,
	custom_minirootfs: Option<Vec<u8>>,
	custom_vmlinuz: Option<Vec<u8>>,
	custom_ovmf: Option<Vec<u8>>,
	ssh_keys: Vec<String>,
	ssh_forward: Option<(u16, u16)>,
	default_cpus: u32,
	default_memory: String,
	bundle_runner: bool,
	extra_files: Vec<(PathBuf, String)>,
	extra_kernel_modules: Vec<PathBuf>,
	kernel_modules_dir: Option<PathBuf>,
	kernel_version: Option<String>,
	kernel_abi: Option<String>,
	ovmf_version: Option<String>,
}

impl AlpineBuilder {
	/// Set the Alpine major version (e.g. `"3.21"`).
	/// Defaults to `"3.21"` or `TDX_IMAGE_ALPINE_VERSION` env var.
	#[must_use]
	pub fn with_major_version(mut self, version: &str) -> Self {
		self.alpine_major = Some(version.to_string());
		self
	}

	/// Set the Alpine minor/point release (e.g. `"0"`).
	/// Defaults to `"0"` or `TDX_IMAGE_ALPINE_MINOR` env var.
	#[must_use]
	pub fn with_minor_version(mut self, version: &str) -> Self {
		self.alpine_minor = Some(version.to_string());
		self
	}

	/// Provide a custom Alpine minirootfs tarball (in-memory bytes).
	/// When set, skips downloading from Alpine CDN.
	#[must_use]
	pub fn with_custom_minirootfs(mut self, bytes: &[u8]) -> Self {
		self.custom_minirootfs = Some(bytes.to_vec());
		self
	}

	/// Provide a custom vmlinuz kernel image (in-memory bytes).
	/// When set, skips auto-downloading the Ubuntu kernel.
	#[must_use]
	pub fn with_custom_vmlinuz(mut self, bytes: &[u8]) -> Self {
		self.custom_vmlinuz = Some(bytes.to_vec());
		self
	}

	/// Provide a custom OVMF.fd firmware (in-memory bytes).
	/// When set, skips auto-downloading OVMF.
	#[must_use]
	pub fn with_custom_ovmf(mut self, bytes: &[u8]) -> Self {
		self.custom_ovmf = Some(bytes.to_vec());
		self
	}

	/// Enable SSH access in the guest by forwarding a host port to the
	/// guest SSH daemon.
	///
	/// `host_port` is the port on the host, `guest_port` is the port
	/// inside the VM (typically 22). Calling this also adds the
	/// `hostfwd` rule to the generated QEMU launch scripts.
	///
	/// Use [`with_ssh_key`](Self::with_ssh_key) to authorise public
	/// keys.
	///
	/// # Example
	///
	/// ```rust,ignore
	/// .with_ssh_forward(2222, 22)
	/// ```
	#[must_use]
	pub const fn with_ssh_forward(
		mut self,
		host_port: u16,
		guest_port: u16,
	) -> Self {
		self.ssh_forward = Some((host_port, guest_port));
		self
	}

	/// Add an SSH public key to the guest's `/root/.ssh/authorized_keys`.
	///
	/// Can be called multiple times to authorise several keys.
	/// Has no effect unless [`with_ssh_forward`](Self::with_ssh_forward)
	/// is also called.
	#[must_use]
	pub fn with_ssh_key(mut self, pubkey: &str) -> Self {
		self.ssh_keys.push(pubkey.to_string());
		self
	}

	/// Set the default vCPU count for the QEMU launch script.
	/// Defaults to 4.
	#[must_use]
	pub const fn with_default_cpu_count(mut self, count: u32) -> Self {
		self.default_cpus = count;
		self
	}

	/// Set the default memory size for the QEMU launch script
	/// (e.g. `"4G"`, `"8G"`). Defaults to `"4G"`.
	#[must_use]
	pub fn with_default_memory_size(mut self, size: &str) -> Self {
		self.default_memory = size.to_string();
		self
	}

	/// Include the mosaik bundle runner in the guest image.
	/// This is the default.
	#[must_use]
	pub const fn with_bundle_runner(mut self) -> Self {
		self.bundle_runner = true;
		self
	}

	/// Exclude the mosaik bundle runner from the guest image.
	#[must_use]
	pub const fn without_bundle_runner(mut self) -> Self {
		self.bundle_runner = false;
		self
	}

	/// Add an extra file to bundle into the initramfs.
	/// `host_path` is the path on the build host, `guest_path` is the
	/// absolute path inside the guest (leading `/` is stripped).
	#[must_use]
	pub fn with_extra_file(
		mut self,
		host_path: impl Into<PathBuf>,
		guest_path: &str,
	) -> Self {
		self.extra_files.push((
			host_path.into(),
			guest_path.trim_start_matches('/').to_string(),
		));
		self
	}

	/// Add a kernel module (.ko file) to bundle into the initramfs.
	#[must_use]
	pub fn with_kernel_module(mut self, path: impl Into<PathBuf>) -> Self {
		self.extra_kernel_modules.push(path.into());
		self
	}

	/// Set the directory to auto-discover TDX kernel modules from.
	/// Equivalent to `TDX_IMAGE_KERNEL_MODULES_DIR`.
	#[must_use]
	pub fn with_kernel_modules_dir(mut self, path: impl Into<PathBuf>) -> Self {
		self.kernel_modules_dir = Some(path.into());
		self
	}

	/// Set the Ubuntu kernel version for auto-download
	/// (e.g. `"6.8.0-55"`). Defaults to `"6.8.0-55"`.
	#[must_use]
	pub fn with_kernel_version(mut self, version: &str) -> Self {
		self.kernel_version = Some(version.to_string());
		self
	}

	/// Set the Ubuntu kernel ABI number for auto-download
	/// (e.g. `"57"`). Defaults to `"57"`.
	#[must_use]
	pub fn with_kernel_abi(mut self, abi: &str) -> Self {
		self.kernel_abi = Some(abi.to_string());
		self
	}

	/// Set the OVMF version for auto-download
	/// (e.g. `"2024.02-3+tdx1.0"`).
	#[must_use]
	pub fn with_ovmf_version(mut self, version: &str) -> Self {
		self.ovmf_version = Some(version.to_string());
		self
	}

	/// Build the TDX guest image.
	///
	/// # Panics
	///
	/// Panics if any required tool (`curl`, `tar`, `gzip`, `ar`,
	/// etc.) is missing or if cross-compilation fails.
	pub fn build(self) {
		// --- Recursion guard -----------------------------------------------
		if env::var("__TDX_IMAGE_INNER_BUILD").is_ok() {
			return;
		}

		println!("cargo:rerun-if-env-changed=TDX_IMAGE_SKIP");
		println!("cargo:rerun-if-env-changed=TDX_IMAGE_ALPINE_VERSION");
		println!("cargo:rerun-if-env-changed=TDX_IMAGE_ALPINE_MINOR");
		println!("cargo:rerun-if-env-changed=TDX_IMAGE_EXTRA_FILES");
		println!("cargo:rerun-if-env-changed=TDX_IMAGE_KERNEL_MODULES");

		if env_or("TDX_IMAGE_SKIP", "0") == "1" {
			eprintln!("TDX_IMAGE_SKIP=1 — skipping initramfs build");
			return;
		}

		let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
		let cache_dir = out_dir.join("tdx-image-cache");
		fs::create_dir_all(&cache_dir).unwrap();

		let crate_name = env::var("CARGO_PKG_NAME").unwrap();
		let profile = detect_profile();
		let target_dir = find_target_dir();

		// Resolve Alpine version: builder field > env var > default
		let alpine_ver = self
			.alpine_major
			.unwrap_or_else(|| env_or("TDX_IMAGE_ALPINE_VERSION", "3.21"));
		let alpine_minor = self
			.alpine_minor
			.unwrap_or_else(|| env_or("TDX_IMAGE_ALPINE_MINOR", "0"));

		let output_filename = format!("{crate_name}-initramfs.cpio.gz");
		let final_path = target_dir.join(profile).join(&output_filename);

		eprintln!("==> TDX initramfs: profile={profile}, crate={crate_name}");
		eprintln!("    output = {}", final_path.display());

		// ---------------------------------------------------------------
		// 1. Cross-compile for x86_64-unknown-linux-musl
		// ---------------------------------------------------------------
		let binary_data = cross_compile_musl(&crate_name, profile, &target_dir);

		// ---------------------------------------------------------------
		// 2. Download / use Alpine minirootfs
		// ---------------------------------------------------------------
		let alpine_tar_path = self.custom_minirootfs.as_ref().map_or_else(
			|| {
				eprintln!("==> Downloading Alpine minirootfs...");
				let alpine_tar_name = format!(
					"alpine-minirootfs-{alpine_ver}.{alpine_minor}-x86_64.tar.gz"
				);
				let alpine_url = format!(
				"https://dl-cdn.alpinelinux.org/alpine/v{alpine_ver}/\
				 releases/x86_64/{alpine_tar_name}"
			);
				download_cached(&alpine_url, &cache_dir, &alpine_tar_name)
			},
			|custom_minirootfs| {
				// Write custom minirootfs to cache so tar helpers work
				let path = cache_dir.join("custom-minirootfs.tar.gz");
				fs::write(&path, custom_minirootfs).unwrap();
				path
			},
		);

		// ---------------------------------------------------------------
		// 3. Extract busybox + musl libs
		// ---------------------------------------------------------------
		eprintln!("==> Extracting busybox...");
		let busybox_data =
			extract_file_from_tar_gz(&alpine_tar_path, "./bin/busybox");

		assert!(
			!busybox_data.is_empty(),
			"Failed to extract bin/busybox from Alpine minirootfs"
		);

		eprintln!(
			"  [ok] busybox ({:.0} KB)",
			busybox_data.len() as f64 / 1024.0
		);

		let tar_entries = list_tar_gz(&alpine_tar_path);
		let musl_libs: Vec<_> = tar_entries
			.iter()
			.filter(|e| {
				e.starts_with("lib/ld-musl") || e.starts_with("lib/libc.musl")
			})
			.cloned()
			.collect();

		// ---------------------------------------------------------------
		// 4. Generate /init script
		// ---------------------------------------------------------------
		eprintln!("==> Generating /init...");
		let init_script = generate_init_script(&crate_name, &self.ssh_keys);

		// ---------------------------------------------------------------
		// 4b. Obtain TDX-enabled kernel + modules
		// ---------------------------------------------------------------
		println!("cargo:rerun-if-env-changed=TDX_IMAGE_KERNEL");
		println!("cargo:rerun-if-env-changed=TDX_IMAGE_KERNEL_VERSION");

		let kernel_cache_dir = cache_dir.join("kernel");
		fs::create_dir_all(&kernel_cache_dir).unwrap();

		let mut auto_modules_dir: Option<PathBuf> = None;
		let mut kernel_vmlinuz: Option<PathBuf> = None;

		if let Some(ref custom_vmlinuz) = self.custom_vmlinuz {
			let path = kernel_cache_dir.join("custom-vmlinuz");
			fs::write(&path, custom_vmlinuz).unwrap();
			kernel_vmlinuz = Some(path);
		} else if let Ok(kernel_path) = env::var("TDX_IMAGE_KERNEL") {
			eprintln!("==> Using user-provided kernel: {kernel_path}");
			kernel_vmlinuz = Some(PathBuf::from(&kernel_path));
		} else {
			let (vmlinuz, modules_dir) = auto_download_kernel(
				&kernel_cache_dir,
				self.kernel_version.as_deref(),
				self.kernel_abi.as_deref(),
			);
			kernel_vmlinuz = vmlinuz;
			auto_modules_dir = modules_dir;
		}

		// ---------------------------------------------------------------
		// 5. Assemble CPIO
		// ---------------------------------------------------------------
		eprintln!("==> Assembling CPIO...");

		let cpio_path = out_dir.join("initramfs.cpio");
		let cpio_file = BufWriter::new(File::create(&cpio_path).unwrap());
		let mut cpio = CpioWriter::new(cpio_file);

		for dir in &[
			"bin",
			"sbin",
			"usr",
			"usr/bin",
			"lib",
			"lib/modules",
			"proc",
			"sys",
			"sys/kernel",
			"sys/kernel/config",
			"dev",
			"dev/pts",
			"dev/shm",
			"tmp",
			"run",
			"etc",
			"var",
			"var/log",
		] {
			cpio.add_dir(dir).unwrap();
		}

		cpio.add_file("init", init_script.as_bytes(), true).unwrap();
		cpio.add_file("bin/busybox", &busybox_data, true).unwrap();

		for applet in &[
			"sh", "mount", "mkdir", "modprobe", "echo", "insmod", "cat", "ls", "ip",
			"udhcpc", "sleep", "grep", "lsmod", "dmesg",
		] {
			cpio
				.add_symlink(&format!("bin/{applet}"), "busybox")
				.unwrap();
		}
		cpio.add_symlink("sbin/modprobe", "../bin/busybox").unwrap();

		for lib_entry in &musl_libs {
			let data =
				extract_file_from_tar_gz(&alpine_tar_path, &format!("./{lib_entry}"));
			if !data.is_empty() {
				eprintln!("  [lib] /{lib_entry}");
				cpio.add_file(lib_entry, &data, true).unwrap();
			}
		}

		cpio
			.add_file(&format!("usr/bin/{crate_name}"), &binary_data, true)
			.unwrap();

		// Extra files from builder API
		for (host_path, guest_path) in &self.extra_files {
			eprintln!("  [extra] {} → /{guest_path}", host_path.display());
			println!("cargo:rerun-if-changed={}", host_path.display());
			let data = fs::read(host_path).unwrap_or_else(|e| {
				panic!("Failed to read {}: {e}", host_path.display())
			});
			cpio.add_file(guest_path, &data, false).unwrap();
		}

		// Extra files from env var
		if let Ok(extras) = env::var("TDX_IMAGE_EXTRA_FILES") {
			let parts: Vec<&str> = extras.split(':').collect();
			for pair in parts.chunks(2) {
				if pair.len() == 2 {
					let host_path = pair[0];
					let guest_path = pair[1].trim_start_matches('/');
					eprintln!("  [extra] {host_path} → /{guest_path}");
					println!("cargo:rerun-if-changed={host_path}");
					let data = fs::read(host_path)
						.unwrap_or_else(|e| panic!("Failed to read {host_path}: {e}"));
					cpio.add_file(guest_path, &data, false).unwrap();
				}
			}
		}

		// Kernel modules from builder API
		for module_path in &self.extra_kernel_modules {
			let filename = module_path.file_name().unwrap().to_str().unwrap();
			let guest_path = format!("lib/modules/{filename}");
			eprintln!("  [module] {} → /{guest_path}", module_path.display());
			println!("cargo:rerun-if-changed={}", module_path.display());
			let data = fs::read(module_path).unwrap_or_else(|e| {
				panic!("Failed to read {}: {e}", module_path.display())
			});
			cpio.add_file(&guest_path, &data, false).unwrap();
		}

		// Kernel modules from env var
		if let Ok(modules) = env::var("TDX_IMAGE_KERNEL_MODULES") {
			for module_path in modules.split(':').filter(|s| !s.is_empty()) {
				let filename = Path::new(module_path)
					.file_name()
					.unwrap()
					.to_str()
					.unwrap();
				let guest_path = format!("lib/modules/{filename}");
				eprintln!("  [module] {module_path} → /{guest_path}");
				println!("cargo:rerun-if-changed={module_path}");
				let data = fs::read(module_path)
					.unwrap_or_else(|e| panic!("Failed to read {module_path}: {e}"));
				cpio.add_file(&guest_path, &data, false).unwrap();
			}
		}

		// Auto-discover TDX kernel modules
		println!("cargo:rerun-if-env-changed=TDX_IMAGE_KERNEL_MODULES_DIR");

		let effective_modules_dir = self
			.kernel_modules_dir
			.clone()
			.or_else(|| {
				env::var("TDX_IMAGE_KERNEL_MODULES_DIR")
					.map(PathBuf::from)
					.ok()
			})
			.or_else(|| auto_modules_dir.clone());

		if let Some(ref modules_dir) = effective_modules_dir {
			bundle_auto_discovered_modules(&mut cpio, modules_dir);
		} else {
			eprintln!(
				"==> No kernel modules directory -- TDX modules will not be bundled."
			);
		}

		// Clean up auto-downloaded modules extract directory
		if let Some(ref dir) = auto_modules_dir {
			let _ = fs::remove_dir_all(dir);
		}

		cpio
			.add_file("etc/passwd", b"root:x:0:0:root:/:/bin/sh\n", false)
			.unwrap();
		cpio.add_file("etc/group", b"root:x:0:\n", false).unwrap();

		cpio
			.add_file("bin/simple.script", UDHCPC_SCRIPT.as_bytes(), true)
			.unwrap();

		cpio.add_file("etc/resolv.conf", b"", false).unwrap();

		let writer = cpio.finish().unwrap();
		writer.into_inner().unwrap().sync_all().unwrap();

		// ---------------------------------------------------------------
		// 6. gzip → final output
		// ---------------------------------------------------------------
		eprintln!("==> Compressing...");
		let gz_path = out_dir.join(&output_filename);

		let gz_file = File::create(&gz_path).unwrap();
		let status = Command::new("gzip")
			.args(["-9", "-c"])
			.arg(&cpio_path)
			.stdout(gz_file)
			.status()
			.expect("failed to run gzip — is it installed?");

		assert!(status.success(), "gzip compression failed");

		fs::create_dir_all(final_path.parent().unwrap()).unwrap();
		fs::copy(&gz_path, &final_path).unwrap();

		// ---------------------------------------------------------------
		// 7. Report initramfs
		// ---------------------------------------------------------------
		let cpio_size = fs::metadata(&cpio_path).map(|m| m.len()).unwrap_or(0);
		let gz_size = fs::metadata(&final_path).map(|m| m.len()).unwrap_or(0);

		println!(
			"cargo:warning=initramfs: {} (cpio {:.1} MB → gz {:.1} MB)",
			final_path.display(),
			cpio_size as f64 / 1_048_576.0,
			gz_size as f64 / 1_048_576.0,
		);

		println!(
			"cargo:rustc-env=TDX_INITRAMFS_PATH={}",
			final_path.display()
		);

		let _ = fs::remove_file(&cpio_path);

		// ---------------------------------------------------------------
		// 8. Copy kernel to output directory
		// ---------------------------------------------------------------
		let kernel_output = target_dir
			.join(profile)
			.join(format!("{crate_name}-vmlinuz"));

		if let Some(ref vmlinuz) = kernel_vmlinuz {
			if vmlinuz.exists() {
				fs::copy(vmlinuz, &kernel_output).unwrap();
				let ksize = fs::metadata(&kernel_output).map(|m| m.len()).unwrap_or(0);
				println!(
					"cargo:warning=kernel: {} ({:.1} MB)",
					kernel_output.display(),
					ksize as f64 / 1_048_576.0,
				);
				println!(
					"cargo:rustc-env=TDX_KERNEL_PATH={}",
					kernel_output.display()
				);
			}
		} else {
			eprintln!(
				"==> No kernel available — set TDX_IMAGE_KERNEL or let auto-download \
				 handle it"
			);
		}

		// ---------------------------------------------------------------
		// 9. Generate launch-tdx.sh helper script
		// ---------------------------------------------------------------
		generate_launch_script(
			&crate_name,
			profile,
			&target_dir,
			self.default_cpus,
			&self.default_memory,
			self.ssh_forward,
		);

		// ---------------------------------------------------------------
		// 10. Obtain OVMF.fd and precompute MRTD
		// ---------------------------------------------------------------
		let ovmf_output = target_dir
			.join(profile)
			.join(format!("{crate_name}-OVMF.fd"));

		let ovmf_data = obtain_ovmf(
			&self.custom_ovmf,
			&kernel_cache_dir,
			self.ovmf_version.as_deref(),
		);

		if let Some(ref data) = ovmf_data {
			fs::write(&ovmf_output, data).unwrap();
			println!(
				"cargo:warning=OVMF: {} ({:.1} MB)",
				ovmf_output.display(),
				data.len() as f64 / 1_048_576.0,
			);

			eprintln!("==> Precomputing MRTD...");
			match mrtd::compute_mrtd(data) {
				Ok(digest) => {
					let hex = digest
						.iter()
						.map(|b| format!("{b:02x}"))
						.collect::<String>();
					println!("cargo:warning=MRTD: {hex}");
					println!("cargo:rustc-env=TDX_EXPECTED_MRTD={hex}");

					let mrtd_path = target_dir
						.join(profile)
						.join(format!("{crate_name}-mrtd.hex"));
					fs::write(&mrtd_path, &hex).unwrap();
					println!("cargo:warning=MRTD written to: {}", mrtd_path.display());
				}
				Err(e) => {
					println!("cargo:warning=MRTD computation failed: {e}");
				}
			}
		}

		// ---------------------------------------------------------------
		// 11. Generate self-extracting run-qemu.sh
		// ---------------------------------------------------------------
		generate_self_extracting_script(
			&crate_name,
			profile,
			&target_dir,
			&out_dir,
			&kernel_output,
			&final_path,
			&ovmf_output,
			self.default_cpus,
			&self.default_memory,
			self.ssh_forward,
		);
	}
}

impl Default for AlpineBuilder {
	fn default() -> Self {
		Self {
			alpine_major: None,
			alpine_minor: None,
			custom_minirootfs: None,
			custom_vmlinuz: None,
			custom_ovmf: None,
			ssh_keys: Vec::new(),
			ssh_forward: None,
			default_cpus: 4,
			default_memory: "4G".to_string(),
			bundle_runner: true,
			extra_files: Vec::new(),
			extra_kernel_modules: Vec::new(),
			kernel_modules_dir: None,
			kernel_version: None,
			kernel_abi: None,
			ovmf_version: None,
		}
	}
}

// ===========================================================================
// CPIO "newc" format writer
// ===========================================================================

struct CpioWriter<W: Write> {
	inner: W,
	ino: u32,
	offset: usize,
}

impl<W: Write> CpioWriter<W> {
	const fn new(writer: W) -> Self {
		Self {
			inner: writer,
			ino: 1,
			offset: 0,
		}
	}

	fn align4(&mut self) -> io::Result<()> {
		let pad = (4 - (self.offset % 4)) % 4;
		if pad > 0 {
			self.inner.write_all(&vec![0u8; pad])?;
			self.offset += pad;
		}
		Ok(())
	}

	fn write_entry(
		&mut self,
		name: &str,
		data: &[u8],
		mode: u32,
	) -> io::Result<()> {
		let namesize = name.len() + 1;
		let filesize = data.len();

		let header = format!(
			"070701{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:\
			 08X}{:08X}{:08X}",
			self.ino,
			mode,
			0u32,
			0u32, // ino, mode, uid, gid
			1u32,
			0u32,
			filesize,
			0u32, // nlink, mtime, filesize, devmajor
			0u32,
			0u32,
			0u32,
			namesize,
			0u32, // devminor, rdevmaj, rdevmin, namesize, check
		);
		debug_assert_eq!(header.len(), 110);

		self.inner.write_all(header.as_bytes())?;
		self.offset += 110;

		self.inner.write_all(name.as_bytes())?;
		self.inner.write_all(&[0])?;
		self.offset += namesize;
		self.align4()?;

		self.inner.write_all(data)?;
		self.offset += filesize;
		self.align4()?;

		self.ino += 1;
		Ok(())
	}

	fn add_file(
		&mut self,
		path: &str,
		data: &[u8],
		executable: bool,
	) -> io::Result<()> {
		self.write_entry(
			path,
			data,
			if executable { 0o100_755 } else { 0o100_6444 },
		)
	}

	fn add_dir(&mut self, path: &str) -> io::Result<()> {
		self.write_entry(path, &[], 0o40755)
	}

	fn add_symlink(&mut self, path: &str, target: &str) -> io::Result<()> {
		self.write_entry(path, target.as_bytes(), 0o120_777)
	}

	fn finish(mut self) -> io::Result<W> {
		self.write_entry("TRAILER!!!", &[], 0)?;
		let pad = (512 - (self.offset % 512)) % 512;
		if pad > 0 {
			self.inner.write_all(&vec![0u8; pad])?;
		}
		Ok(self.inner)
	}
}

// ===========================================================================
// Helpers
// ===========================================================================

fn env_or(key: &str, default: &str) -> String {
	env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Build the QEMU `-netdev user` host-forward fragment from the
/// optional `(host_port, guest_port)` pair.
///
/// When no forwarding is configured the generated scripts fall back to
/// the `$SSH_PORT` env-var (default 10022) → guest :22.
fn ssh_fwd_rule(ssh_forward: Option<(u16, u16)>) -> String {
	match ssh_forward {
		Some((host, guest)) => {
			format!("hostfwd=tcp::{host}-:{guest}")
		}
		None => "hostfwd=tcp::\"$SSH_PORT\"-:22".to_string(),
	}
}

fn download_cached(url: &str, cache_dir: &Path, filename: &str) -> PathBuf {
	let cached = cache_dir.join(filename);
	if cached.exists() {
		eprintln!("  [cached] {}", cached.display());
		return cached;
	}
	eprintln!("  [download] {url}");

	let status = Command::new("curl")
		.args(["-fSL", "--retry", "3", "-o"])
		.arg(&cached)
		.arg(url)
		.status()
		.expect("failed to run curl — is it installed?");

	if !status.success() {
		let _ = fs::remove_file(&cached);
		panic!("curl failed to download {url}");
	}
	cached
}

fn extract_file_from_tar_gz(archive: &Path, inner_path: &str) -> Vec<u8> {
	let output = Command::new("tar")
		.args(["xzf"])
		.arg(archive)
		.args(["--to-stdout", inner_path])
		.output()
		.expect("failed to run tar");

	assert!(
		output.status.success(),
		"tar: failed to extract {inner_path} from {}: {}",
		archive.display(),
		String::from_utf8_lossy(&output.stderr)
	);

	output.stdout
}

fn list_tar_gz(archive: &Path) -> Vec<String> {
	let output = Command::new("tar")
		.args(["tzf"])
		.arg(archive)
		.output()
		.expect("failed to run tar");

	assert!(
		output.status.success(),
		"tar: failed to list {}",
		archive.display()
	);

	String::from_utf8_lossy(&output.stdout)
		.lines()
		.map(|l| l.trim_start_matches("./").to_string())
		.filter(|l| !l.is_empty())
		.collect()
}

fn find_file_starting_with(dir: &Path, prefix: &str) -> Option<PathBuf> {
	fs::read_dir(dir)
		.ok()?
		.filter_map(|e| e.ok())
		.find(|e| {
			e.file_name()
				.to_str()
				.is_some_and(|n| n.starts_with(prefix))
		})
		.map(|e| e.path())
}

fn find_file_recursive(dir: &Path, name_fragment: &str) -> Option<PathBuf> {
	let output = Command::new("find")
		.arg(dir)
		.args(["-name", &format!("{name_fragment}*"), "-type", "f"])
		.output()
		.ok()?;

	if !output.status.success() {
		return None;
	}

	String::from_utf8_lossy(&output.stdout)
		.lines()
		.next()
		.filter(|s| !s.is_empty())
		.map(|s| PathBuf::from(s.trim()))
}

fn detect_profile() -> &'static str {
	let out_dir = env::var("OUT_DIR").unwrap();
	if out_dir.contains("/release/") || out_dir.contains("\\release\\") {
		"release"
	} else {
		"debug"
	}
}

fn find_target_dir() -> PathBuf {
	if let Ok(d) = env::var("CARGO_TARGET_DIR") {
		return PathBuf::from(d);
	}

	let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
	let mut dir = out_dir.as_path();
	loop {
		if dir.file_name().is_some_and(|n| n == "target") {
			return dir.to_path_buf();
		}
		match dir.parent() {
			Some(parent) if parent != dir => dir = parent,
			_ => {
				let mut fallback = out_dir.clone();
				for _ in 0..5 {
					if let Some(p) = fallback.parent() {
						fallback = p.to_path_buf();
					}
				}
				return fallback;
			}
		}
	}
}

// ===========================================================================
// Cross-compilation
// ===========================================================================

fn cross_compile_musl(
	crate_name: &str,
	profile: &str,
	target_dir: &Path,
) -> Vec<u8> {
	let musl_target = "x86_64-unknown-linux-musl";

	eprintln!("==> Cross-compiling {crate_name} for {musl_target}...");

	let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
	let inner_target_dir = target_dir.join("tdx-musl-build");

	let mut cmd =
		Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".to_string()));

	cmd
		.arg("build")
		.arg("--target")
		.arg(musl_target)
		.arg("--manifest-path")
		.arg(manifest_dir.join("Cargo.toml"))
		.arg("--target-dir")
		.arg(&inner_target_dir);

	if profile == "release" {
		cmd.arg("--release");
	}

	cmd.env("__TDX_IMAGE_INNER_BUILD", "1");

	let musl_linker_env = "CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER";
	let musl_cc_env = "CC_x86_64_unknown_linux_musl";

	if env::var(musl_linker_env).is_err() {
		let candidates =
			["x86_64-linux-musl-gcc", "x86_64-linux-gnu-gcc", "musl-gcc"];

		let found = candidates.iter().find(|c| {
			Command::new("which")
				.arg(c)
				.stdout(std::process::Stdio::null())
				.stderr(std::process::Stdio::null())
				.status()
				.map(|s| s.success())
				.unwrap_or(false)
		});

		if let Some(linker) = found {
			eprintln!("  [linker] auto-detected: {linker}");
			cmd.env(musl_linker_env, linker);
			cmd.env(musl_cc_env, linker);
		} else if cfg!(not(target_os = "linux")) {
			panic!(
				"\n═════════════════════════════════════════════════════════════════\\
				 nNo musl cross-linker found in PATH.\n\nInstall one:\n\nmacOS:  brew \
				 install filosottile/musl-cross/musl-cross\nNix: nix-shell -p \
				 pkgsCross.musl64.stdenv.cc\n\nOr set the linker \
				 explicitly:\n\nexport \
				 CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=x86_64-linux-musl-gcc\\
				 n═════════════════════════════════════════════════════════════════\n"
			);
		}
	} else {
		eprintln!(
			"  [linker] using {musl_linker_env}={}",
			env::var(musl_linker_env).unwrap()
		);
		cmd.env(musl_linker_env, env::var(musl_linker_env).unwrap());
		if let Ok(cc) = env::var(musl_cc_env) {
			cmd.env(musl_cc_env, cc);
		}
	}

	eprintln!(
		"  $ cargo build --target {musl_target} --target-dir {} {}",
		inner_target_dir.display(),
		if profile == "release" {
			"--release"
		} else {
			""
		}
	);

	let status = cmd
		.status()
		.expect("failed to invoke cargo for musl cross-build");

	assert!(
		status.success(),
		"\n══════════════════════════════════════════════════════════════════\\
 			 nCross-compilation to {musl_target} failed.\n\nCheck the compiler output \
		 above for details.\nIf the linker failed, ensure your musl \
		 cross-toolchain is correct:\n\nexport \
		 CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=x86_64-linux-musl-gcc\\
 			 n══════════════════════════════════════════════════════════════════\n"
	);

	let musl_binary = inner_target_dir
		.join(musl_target)
		.join(profile)
		.join(crate_name);

	assert!(
		musl_binary.exists(),
		"Cross-build succeeded but binary not found at: {}",
		musl_binary.display()
	);

	println!("cargo:rerun-if-changed={}", musl_binary.display());

	let binary_data = fs::read(&musl_binary).expect("failed to read musl binary");

	assert!(
		!(binary_data.len() < 4 || &binary_data[..4] != b"\x7fELF"),
		"Binary at {} is not a Linux ELF",
		musl_binary.display()
	);

	eprintln!(
		"  [ok] {} ({:.1} MB)",
		musl_binary.display(),
		binary_data.len() as f64 / 1_048_576.0
	);

	binary_data
}

// ===========================================================================
// Kernel auto-download
// ===========================================================================

fn auto_download_kernel(
	kernel_cache_dir: &Path,
	kernel_version: Option<&str>,
	kernel_abi: Option<&str>,
) -> (Option<PathBuf>, Option<PathBuf>) {
	let kver = kernel_version.map_or_else(
		|| env_or("TDX_IMAGE_KERNEL_VERSION", "6.8.0-55"),
		|v| v.to_string(),
	);
	let kabi = kernel_abi
		.map_or_else(|| env_or("TDX_IMAGE_KERNEL_ABI", "57"), |v| v.to_string());

	let image_deb_name =
		format!("linux-image-unsigned-{kver}-generic_{kver}.{kabi}_amd64.deb");
	let modules_base_deb_name =
		format!("linux-modules-{kver}-generic_{kver}.{kabi}_amd64.deb");
	let modules_extra_deb_name =
		format!("linux-modules-extra-{kver}-generic_{kver}.{kabi}_amd64.deb");

	let base_url = "https://archive.ubuntu.com/ubuntu/pool/main/l/linux";

	eprintln!("==> Auto-downloading Ubuntu {kver}-generic kernel...");
	eprintln!("    Set TDX_IMAGE_KERNEL=/path/to/vmlinuz to skip download");

	let image_deb = download_cached(
		&format!("{base_url}/{image_deb_name}"),
		kernel_cache_dir,
		&image_deb_name,
	);

	// Extract vmlinuz from the .deb
	let extract_dir = kernel_cache_dir.join("extract-image");
	let _ = fs::remove_dir_all(&extract_dir);
	fs::create_dir_all(&extract_dir).unwrap();

	eprintln!("  [extract] {image_deb_name}...");
	let _ = Command::new("ar")
		.args(["x"])
		.arg(&image_deb)
		.current_dir(&extract_dir)
		.status()
		.expect("failed to run `ar` — install binutils");

	if let Some(data_tar) = find_file_starting_with(&extract_dir, "data.tar") {
		let _ = Command::new("tar")
			.args(["xf"])
			.arg(&data_tar)
			.args(["-C"])
			.arg(&extract_dir)
			.status();
	}

	let found_vmlinuz = find_file_recursive(&extract_dir, "vmlinuz");
	let mut kernel_vmlinuz = None;
	if let Some(vml) = &found_vmlinuz {
		let cached_vmlinuz = kernel_cache_dir.join("vmlinuz");
		fs::copy(vml, &cached_vmlinuz).unwrap();
		kernel_vmlinuz = Some(cached_vmlinuz);
		eprintln!(
			"  [ok] vmlinuz extracted ({:.1} MB)",
			fs::metadata(vml).unwrap().len() as f64 / 1_048_576.0
		);
	} else {
		eprintln!("  [warn] vmlinuz not found in {image_deb_name}");
	}
	let _ = fs::remove_dir_all(&extract_dir);

	// Download and extract module .deb packages
	let modules_extract_dir = kernel_cache_dir.join("extract-modules");
	let _ = fs::remove_dir_all(&modules_extract_dir);
	fs::create_dir_all(&modules_extract_dir).unwrap();

	let module_debs = [
		(&modules_base_deb_name, "linux-modules"),
		(&modules_extra_deb_name, "linux-modules-extra"),
	];

	for (deb_name, label) in &module_debs {
		eprintln!("  [download] {label}: {deb_name}...");
		let deb_path = download_cached(
			&format!("{base_url}/{deb_name}"),
			kernel_cache_dir,
			deb_name,
		);

		let tmp_dir = kernel_cache_dir.join(format!("tmp-{label}"));
		let _ = fs::remove_dir_all(&tmp_dir);
		fs::create_dir_all(&tmp_dir).unwrap();

		let ar_status = Command::new("ar")
			.args(["x"])
			.arg(&deb_path)
			.current_dir(&tmp_dir)
			.status();

		if ar_status.map(|s| s.success()).unwrap_or(false) {
			if let Some(data_tar) = find_file_starting_with(&tmp_dir, "data.tar") {
				let _ = Command::new("tar")
					.args(["xf"])
					.arg(&data_tar)
					.args(["-C"])
					.arg(&modules_extract_dir)
					.status();
				eprintln!("  [ok] {label} extracted");
			} else {
				eprintln!("  [warn] no data.tar.* in {deb_name}");
			}
		} else {
			eprintln!("  [warn] ar failed on {deb_name}");
		}

		let _ = fs::remove_dir_all(&tmp_dir);
	}

	(kernel_vmlinuz, Some(modules_extract_dir))
}

// ===========================================================================
// Module auto-discovery
// ===========================================================================

fn bundle_auto_discovered_modules<W: Write>(
	cpio: &mut CpioWriter<W>,
	modules_dir: &Path,
) {
	eprintln!(
		"==> Auto-discovering TDX kernel modules from {}...",
		modules_dir.display()
	);

	let wanted_modules = [
		"configfs",
		"tsm",
		"tdx_guest",
		"tdx-guest",
		"vsock",
		"vmw_vsock_virtio_transport",
		"vmw_vsock_virtio_transport_common",
	];

	for module_name in &wanted_modules {
		let find_output = Command::new("find")
			.arg(modules_dir)
			.args([
				"-name",
				&format!("{module_name}.ko"),
				"-o",
				"-name",
				&format!("{module_name}.ko.zst"),
				"-o",
				"-name",
				&format!("{module_name}.ko.xz"),
			])
			.output();

		let found_path = match find_output {
			Ok(output) if output.status.success() => {
				String::from_utf8_lossy(&output.stdout)
					.lines()
					.next()
					.filter(|s| !s.is_empty())
					.map(|s| s.to_string())
			}
			_ => None,
		};

		if let Some(ko_path) = found_path {
			let ko_filename =
				Path::new(&ko_path).file_name().unwrap().to_str().unwrap();

			let (data, guest_name) = if std::path::Path::new(ko_filename)
				.extension()
				.is_some_and(|ext| ext.eq_ignore_ascii_case("zst"))
			{
				let out = Command::new("zstd")
					.args(["-d", "-c"])
					.arg(&ko_path)
					.output()
					.expect(
						"failed to run zstd — install zstd to decompress kernel modules",
					);
				assert!(out.status.success(), "zstd failed to decompress {ko_path}");
				(
					out.stdout,
					ko_filename.strip_suffix(".zst").unwrap().to_string(),
				)
			} else if std::path::Path::new(ko_filename)
				.extension()
				.is_some_and(|ext| ext.eq_ignore_ascii_case("xz"))
			{
				let out = Command::new("xz")
					.args(["-d", "-c"])
					.arg(&ko_path)
					.output()
					.expect("failed to run xz — install xz to decompress kernel modules");

				assert!(out.status.success(), "xz failed to decompress {ko_path}");

				(
					out.stdout,
					ko_filename.strip_suffix(".xz").unwrap().to_string(),
				)
			} else {
				let data = fs::read(&ko_path)
					.unwrap_or_else(|e| panic!("Failed to read {ko_path}: {e}"));
				(data, ko_filename.to_string())
			};

			let guest_path = format!("lib/modules/{guest_name}");
			eprintln!(
				"  [module] {ko_path} -> /{guest_path} ({:.0} KB)",
				data.len() as f64 / 1024.0
			);
			cpio.add_file(&guest_path, &data, false).unwrap();
		} else {
			eprintln!("  [module] {module_name}.ko not found (may be built-in)");
		}
	}
}

// ===========================================================================
// Init script generation
// ===========================================================================

fn generate_init_script(crate_name: &str, ssh_keys: &[String]) -> String {
	let ssh_block = if ssh_keys.is_empty() {
		String::new()
	} else {
		include_str!("templates/alpine/init-ssh.sh")
			.replace("{{SSH_KEYS}}", &ssh_keys.join("\n"))
	};

	include_str!("templates/alpine/init.sh")
		.replace("{{CRATE_NAME}}", crate_name)
		.replace("{{SSH_BLOCK}}", &ssh_block)
}

// ===========================================================================
// OVMF acquisition
// ===========================================================================

fn obtain_ovmf(
	custom_ovmf: &Option<Vec<u8>>,
	kernel_cache_dir: &Path,
	ovmf_version: Option<&str>,
) -> Option<Vec<u8>> {
	println!("cargo:rerun-if-env-changed=TDX_IMAGE_OVMF");
	println!("cargo:rerun-if-env-changed=TDX_IMAGE_OVMF_VERSION");

	if let Some(data) = custom_ovmf {
		eprintln!("==> Using builder-provided custom OVMF");
		return Some(data.clone());
	}

	if let Ok(ovmf_path) = env::var("TDX_IMAGE_OVMF") {
		eprintln!("==> Using user-provided OVMF: {ovmf_path}");
		println!("cargo:rerun-if-changed={ovmf_path}");
		return Some(fs::read(&ovmf_path).unwrap_or_else(|e| {
			panic!("Failed to read OVMF.fd at {ovmf_path}: {e}")
		}));
	}

	eprintln!("==> Auto-downloading OVMF.fd...");

	let version = ovmf_version.map_or_else(
		|| env_or("TDX_IMAGE_OVMF_VERSION", "2024.02-3+tdx1.0"),
		|v| v.to_string(),
	);

	let ovmf_urls = if version.contains("tdx") {
		vec![format!(
			"https://ppa.launchpadcontent.net/kobuk-team/\
			 tdx-release/ubuntu/pool/main/e/edk2/\
			 ovmf_{version}_all.deb"
		)]
	} else {
		vec![format!(
			"https://archive.ubuntu.com/ubuntu/pool/main/e/edk2/\
			 ovmf_{version}_all.deb"
		)]
	};

	let mut downloaded = None;
	let ovmf_deb_name = format!("ovmf_{version}_all.deb");

	for url in &ovmf_urls {
		let cached = kernel_cache_dir.join(&ovmf_deb_name);
		if cached.exists() {
			eprintln!("  [cached] {}", cached.display());
			downloaded = Some(cached);
			break;
		}
		eprintln!("  [download] {url}");
		let status = Command::new("curl")
			.args(["-fSL", "--retry", "3", "-o"])
			.arg(&cached)
			.arg(url)
			.status();
		if status.map(|s| s.success()).unwrap_or(false) {
			downloaded = Some(cached);
			break;
		}
		let _ = fs::remove_file(&cached);
		eprintln!("  [warn] Failed to download from {url}");
	}

	let deb_path = downloaded?;

	let ovmf_extract = kernel_cache_dir.join("extract-ovmf");
	let _ = fs::remove_dir_all(&ovmf_extract);
	fs::create_dir_all(&ovmf_extract).unwrap();

	let _ = Command::new("ar")
		.args(["x"])
		.arg(&deb_path)
		.current_dir(&ovmf_extract)
		.status();

	if let Some(data_tar) = find_file_starting_with(&ovmf_extract, "data.tar") {
		let _ = Command::new("tar")
			.args(["xf"])
			.arg(&data_tar)
			.args(["-C"])
			.arg(&ovmf_extract)
			.status();
	}

	let found = find_file_recursive(&ovmf_extract, "OVMF.fd");
	let result = found.as_ref().map_or_else(
		|| {
			eprintln!("  [warn] OVMF.fd not found in .deb");
			None
		},
		|ovmf_path| {
			let data = fs::read(ovmf_path).ok();
			if let Some(ref d) = data {
				eprintln!(
					"  [ok] OVMF.fd extracted ({:.1} MB)",
					d.len() as f64 / 1_048_576.0
				);
			}
			data
		},
	);

	let _ = fs::remove_dir_all(&ovmf_extract);
	result
}

// ===========================================================================
// Launch script generation
// ===========================================================================

fn generate_launch_script(
	crate_name: &str,
	profile: &str,
	target_dir: &Path,
	cpus: u32,
	memory: &str,
	ssh_forward: Option<(u16, u16)>,
) {
	let ssh_fwd = ssh_fwd_rule(ssh_forward);
	let launch_script = include_str!("templates/alpine/launch-tdx.sh")
		.replace("{{CRATE_NAME}}", crate_name)
		.replace("{{CPUS}}", &cpus.to_string())
		.replace("{{MEMORY}}", memory)
		.replace("{{SSH_FWD}}", &ssh_fwd);

	let launch_script_path = target_dir
		.join(profile)
		.join(format!("{crate_name}-launch-tdx.sh"));
	fs::write(&launch_script_path, &launch_script).unwrap();

	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		let mut perms = fs::metadata(&launch_script_path).unwrap().permissions();
		perms.set_mode(0o755);
		fs::set_permissions(&launch_script_path, perms).unwrap();
	}

	println!(
		"cargo:warning=launch script: {}",
		launch_script_path.display()
	);
}

// ===========================================================================
// Self-extracting script generation
// ===========================================================================

#[expect(clippy::too_many_arguments)]
fn generate_self_extracting_script(
	crate_name: &str,
	profile: &str,
	target_dir: &Path,
	out_dir: &Path,
	kernel_output: &Path,
	final_path: &Path,
	ovmf_output: &Path,
	cpus: u32,
	memory: &str,
	ssh_forward: Option<(u16, u16)>,
) {
	let run_qemu_path = target_dir
		.join(profile)
		.join(format!("{crate_name}-run-qemu.sh"));

	if !kernel_output.exists() || !final_path.exists() {
		eprintln!("==> Skipping run-qemu.sh (kernel or initramfs not available)");
		return;
	}

	eprintln!("==> Generating self-extracting {crate_name}-run-qemu.sh...");

	let bundle_dir = out_dir.join("bundle");
	let _ = fs::remove_dir_all(&bundle_dir);
	fs::create_dir_all(&bundle_dir).unwrap();

	fs::copy(kernel_output, bundle_dir.join("vmlinuz")).unwrap();
	fs::copy(final_path, bundle_dir.join("initramfs.cpio.gz")).unwrap();
	if ovmf_output.exists() {
		fs::copy(ovmf_output, bundle_dir.join("OVMF.fd")).unwrap();
	}

	let tar_path = out_dir.join("payload.tar.gz");
	let mut tar_args = vec!["vmlinuz", "initramfs.cpio.gz"];
	if ovmf_output.exists() {
		tar_args.push("OVMF.fd");
	}

	let tar_status = Command::new("tar")
		.args(["czf"])
		.arg(&tar_path)
		.args(["-C"])
		.arg(&bundle_dir)
		.args(&tar_args)
		.status()
		.expect("failed to run tar");

	if !tar_status.success() {
		eprintln!("  [warn] Failed to create tar payload — skipping run-qemu.sh");
		let _ = fs::remove_dir_all(&bundle_dir);
		return;
	}

	let payload = fs::read(&tar_path).unwrap();

	let ssh_fwd = ssh_fwd_rule(ssh_forward);
	let header = include_str!("templates/alpine/run-qemu.sh")
		.replace("{{CRATE_NAME}}", crate_name)
		.replace("{{CPUS}}", &cpus.to_string())
		.replace("{{MEMORY}}", memory)
		.replace("{{SSH_FWD}}", &ssh_fwd);

	let mut out_file = File::create(&run_qemu_path).unwrap();
	out_file.write_all(header.as_bytes()).unwrap();
	out_file.write_all(&payload).unwrap();
	out_file.sync_all().unwrap();

	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		let mut perms = fs::metadata(&run_qemu_path).unwrap().permissions();
		perms.set_mode(0o755);
		fs::set_permissions(&run_qemu_path, perms).unwrap();
	}

	let run_qemu_size =
		fs::metadata(&run_qemu_path).map(|m| m.len()).unwrap_or(0);

	println!(
		"cargo:warning=run-qemu: {} ({:.1} MB — single-file deployment)",
		run_qemu_path.display(),
		run_qemu_size as f64 / 1_048_576.0,
	);

	let _ = fs::remove_dir_all(&bundle_dir);
	let _ = fs::remove_file(out_dir.join("payload.tar.gz"));
}

// ===========================================================================
// udhcpc script
// ===========================================================================

const UDHCPC_SCRIPT: &str = include_str!("templates/alpine/udhcpc.sh");

// ===========================================================================
// MRTD precomputation
// ===========================================================================

mod mrtd {
	use std::process::Command;

	const PAGE_SIZE: usize = 4096;
	const CHUNK_SIZE: usize = 256;
	const CHUNKS_PER_PAGE: usize = PAGE_SIZE / CHUNK_SIZE;
	const SHA384_LEN: usize = 48;

	const TDVF_METADATA_GUID: [u8; 16] = [
		0x35, 0x65, 0x7a, 0xe4, 0x4a, 0x98, 0x98, 0x47, 0x86, 0x5e, 0x46, 0x85,
		0xa7, 0xbf, 0x8e, 0xc2,
	];

	#[derive(Debug)]
	struct TdvfSection {
		data_offset: u32,
		raw_data_size: u32,
		memory_address: u64,
		memory_data_size: u64,
		section_type: u32,
		attributes: u32,
	}

	const SECTION_TYPE_BFV: u32 = 0;
	const SECTION_TYPE_CFV: u32 = 1;
	const SECTION_TYPE_TD_HOB: u32 = 2;
	const SECTION_TYPE_TEMP_MEM: u32 = 3;
	const ATTR_MR_EXTEND: u32 = 0x0000_0001;
	const ATTR_PAGE_AUG: u32 = 0x0000_0002;

	fn parse_tdvf_sections(ovmf: &[u8]) -> Result<Vec<TdvfSection>, String> {
		let len = ovmf.len();
		if len < 0x28 {
			return Err("OVMF.fd too small".into());
		}

		let offset_location = len - 0x20;
		let descriptor_offset = u32::from_le_bytes(
			ovmf[offset_location..offset_location + 4]
				.try_into()
				.unwrap(),
		) as usize;

		if descriptor_offset == 0 || descriptor_offset > len {
			return Err(format!(
				"Invalid TDVF descriptor offset: 0x{descriptor_offset:x} (file size: \
				 0x{len:x})"
			));
		}

		let desc_start = len - descriptor_offset;
		let desc = &ovmf[desc_start..];

		if desc.len() < 28 {
			return Err("TDVF descriptor region too small".into());
		}

		let guid = &desc[0..16];
		if guid != TDVF_METADATA_GUID {
			return Err(format!("TDVF metadata GUID mismatch: got {guid:02x?}"));
		}

		let _length = u32::from_le_bytes(desc[16..20].try_into().unwrap());
		let version = u32::from_le_bytes(desc[20..24].try_into().unwrap());
		let num_sections = u32::from_le_bytes(desc[24..28].try_into().unwrap());

		eprintln!("  [tdvf] version={version}, sections={num_sections}");

		let mut sections = Vec::new();
		for i in 0..num_sections as usize {
			let base = 28 + i * 32;
			if base + 32 > desc.len() {
				return Err(format!("TDVF section {i} extends past descriptor"));
			}
			let s = &desc[base..base + 32];
			let section = TdvfSection {
				data_offset: u32::from_le_bytes(s[0..4].try_into().unwrap()),
				raw_data_size: u32::from_le_bytes(s[4..8].try_into().unwrap()),
				memory_address: u64::from_le_bytes(s[8..16].try_into().unwrap()),
				memory_data_size: u64::from_le_bytes(s[16..24].try_into().unwrap()),
				section_type: u32::from_le_bytes(s[24..28].try_into().unwrap()),
				attributes: u32::from_le_bytes(s[28..32].try_into().unwrap()),
			};
			let type_name = match section.section_type {
				SECTION_TYPE_BFV => "BFV",
				SECTION_TYPE_CFV => "CFV",
				SECTION_TYPE_TD_HOB => "TD_HOB",
				SECTION_TYPE_TEMP_MEM => "TEMP_MEM",
				_ => "UNKNOWN",
			};
			let mr = if section.attributes & ATTR_MR_EXTEND != 0 {
				" [EXTENDMR]"
			} else {
				""
			};
			let aug = if section.attributes & ATTR_PAGE_AUG != 0 {
				" [PAGE_AUG]"
			} else {
				""
			};
			eprintln!(
				"  [tdvf]   section[{i}]: type={type_name} gpa=0x{:x} size=0x{:x} \
				 raw_offset=0x{:x} raw_size=0x{:x}{mr}{aug}",
				section.memory_address,
				section.memory_data_size,
				section.data_offset,
				section.raw_data_size,
			);
			sections.push(section);
		}

		Ok(sections)
	}

	fn sha384(data: &[u8]) -> Result<[u8; SHA384_LEN], String> {
		use std::{io::Write, process::Stdio};

		let mut child = Command::new("openssl")
			.args(["dgst", "-sha384", "-binary"])
			.stdin(Stdio::piped())
			.stdout(Stdio::piped())
			.stderr(Stdio::piped())
			.spawn()
			.map_err(|e| format!("failed to run openssl: {e}"))?;

		child
			.stdin
			.as_mut()
			.unwrap()
			.write_all(data)
			.map_err(|e| format!("failed to write to openssl stdin: {e}"))?;

		let output = child
			.wait_with_output()
			.map_err(|e| format!("openssl failed: {e}"))?;

		if !output.status.success() {
			return Err(format!(
				"openssl sha384 failed: {}",
				String::from_utf8_lossy(&output.stderr)
			));
		}

		if output.stdout.len() != SHA384_LEN {
			return Err(format!(
				"openssl sha384 returned {} bytes, expected {SHA384_LEN}",
				output.stdout.len()
			));
		}

		let mut hash = [0u8; SHA384_LEN];
		hash.copy_from_slice(&output.stdout);
		Ok(hash)
	}

	pub fn compute_mrtd(ovmf: &[u8]) -> Result<[u8; SHA384_LEN], String> {
		let sections = parse_tdvf_sections(ovmf)?;
		let mut mrtd = [0u8; SHA384_LEN];

		for section in &sections {
			let extend_mr = section.attributes & ATTR_MR_EXTEND != 0;
			let page_aug = section.attributes & ATTR_PAGE_AUG != 0;

			if !extend_mr || page_aug {
				continue;
			}

			let gpa_base = section.memory_address;
			let mem_size = section.memory_data_size as usize;
			let raw_offset = section.data_offset as usize;
			let raw_size = section.raw_data_size as usize;

			let section_data = if raw_size > 0 && raw_offset + raw_size <= ovmf.len()
			{
				&ovmf[raw_offset..raw_offset + raw_size]
			} else {
				&[] as &[u8]
			};

			let num_pages = mem_size.div_ceil(PAGE_SIZE);

			eprintln!(
				"  [mrtd] Measuring section at GPA 0x{gpa_base:x}, {num_pages} \
				 pages..."
			);

			for page_idx in 0..num_pages {
				let page_gpa = gpa_base + (page_idx as u64) * (PAGE_SIZE as u64);

				// TDH.MEM.PAGE.ADD
				let mut add_buf = [0u8; 128];
				add_buf[..12].copy_from_slice(b"MEM.PAGE.ADD");
				add_buf[16..24].copy_from_slice(&page_gpa.to_le_bytes());

				let mut hash_input = Vec::with_capacity(SHA384_LEN + 128);
				hash_input.extend_from_slice(&mrtd);
				hash_input.extend_from_slice(&add_buf);
				mrtd = sha384(&hash_input)?;

				// TDH.MR.EXTEND for each 256-byte chunk
				let page_data_start = page_idx * PAGE_SIZE;
				let mut page_data = [0u8; PAGE_SIZE];
				if page_data_start < section_data.len() {
					let available = section_data.len() - page_data_start;
					let copy_len = available.min(PAGE_SIZE);
					page_data[..copy_len].copy_from_slice(
						&section_data[page_data_start..page_data_start + copy_len],
					);
				}

				for chunk_idx in 0..CHUNKS_PER_PAGE {
					let chunk_gpa = page_gpa + (chunk_idx as u64) * (CHUNK_SIZE as u64);

					let mut ext_header = [0u8; 128];
					ext_header[..9].copy_from_slice(b"MR.EXTEND");
					ext_header[16..24].copy_from_slice(&chunk_gpa.to_le_bytes());

					let chunk_start = chunk_idx * CHUNK_SIZE;
					let chunk_data = &page_data[chunk_start..chunk_start + CHUNK_SIZE];

					let mut hash_input =
						Vec::with_capacity(SHA384_LEN + 128 + CHUNK_SIZE);
					hash_input.extend_from_slice(&mrtd);
					hash_input.extend_from_slice(&ext_header);
					hash_input.extend_from_slice(chunk_data);
					mrtd = sha384(&hash_input)?;
				}
			}
		}

		let hex = mrtd.iter().map(|b| format!("{b:02x}")).collect::<String>();
		eprintln!("  [mrtd] MRTD = {hex}");

		Ok(mrtd)
	}
}
