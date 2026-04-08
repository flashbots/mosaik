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

use {
	crate::tee::tdx::ticket::Measurement,
	std::{
		env,
		fs::{self, File},
		io::{self, BufWriter, Write},
		path::{Path, PathBuf},
		process::{Command, Stdio},
	},
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
	pub fn build(self) -> Option<BuiltOutput> {
		// --- Recursion guard -----------------------------------------------
		if env::var("__TDX_IMAGE_INNER_BUILD").is_ok() {
			return None;
		}

		println!("cargo:rerun-if-env-changed=TDX_IMAGE_SKIP");
		println!("cargo:rerun-if-env-changed=TDX_IMAGE_ALPINE_VERSION");
		println!("cargo:rerun-if-env-changed=TDX_IMAGE_ALPINE_MINOR");
		println!("cargo:rerun-if-env-changed=TDX_IMAGE_EXTRA_FILES");
		println!("cargo:rerun-if-env-changed=TDX_IMAGE_KERNEL_MODULES");

		if env_or("TDX_IMAGE_SKIP", "0") == "1" {
			eprintln!("TDX_IMAGE_SKIP=1 — skipping initramfs build");
			return None;
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

		// When a custom kernel is provided but no modules directory is
		// specified, still auto-download Ubuntu kernel modules so that
		// TDX attestation drivers (tsm, tdx-guest, vsock, …) are
		// bundled into the initramfs.
		let has_explicit_modules_dir = self.kernel_modules_dir.is_some()
			|| env::var("TDX_IMAGE_KERNEL_MODULES_DIR").is_ok()
			|| !self.extra_kernel_modules.is_empty();

		if auto_modules_dir.is_none() && !has_explicit_modules_dir {
			eprintln!(
				"==> No modules directory — auto-downloading Ubuntu kernel modules \
				 for TDX support..."
			);
			let (_, modules_dir) = auto_download_kernel(
				&kernel_cache_dir,
				self.kernel_version.as_deref(),
				self.kernel_abi.as_deref(),
			);
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
			let n = bundle_auto_discovered_modules(&mut cpio, modules_dir);
			if n == 0 {
				println!(
					"cargo:warning=No TDX kernel modules found in {}. TDX attestation \
					 (configfs-tsm) will not work. Set TDX_IMAGE_KERNEL_MODULES_DIR to \
					 a directory containing the kernel modules.",
					modules_dir.display()
				);
			} else {
				eprintln!("  [ok] {n} kernel module(s) bundled");
			}
		} else {
			println!(
				"cargo:warning=No kernel modules directory — TDX modules will not be \
				 bundled. Set TDX_IMAGE_KERNEL_MODULES_DIR or let auto-download \
				 handle it."
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
			.args(["-9", "-n", "-c"])
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

		let mut mrtd_value = [0u8; 48];
		let mut rtmr1_value = [0u8; 48];
		let mut rtmr2_value = [0u8; 48];

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
					mrtd_value = digest;
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
		// 10b. Precompute RTMR[2] (initrd + cmdline measurement)
		// ---------------------------------------------------------------
		if final_path.exists() {
			eprintln!("==> Precomputing RTMR[2]...");
			let initrd_data = fs::read(&final_path).unwrap();
			let cmdline = "console=ttyS0 ip=dhcp";
			rtmr2_value = mrtd::compute_rtmr2(cmdline, &initrd_data);
			let hex = rtmr2_value
				.iter()
				.map(|b| format!("{b:02x}"))
				.collect::<String>();
			println!("cargo:warning=RTMR[2]: {hex}");
			println!("cargo:rustc-env=TDX_EXPECTED_RTMR2={hex}");

			let rtmr2_path = target_dir
				.join(profile)
				.join(format!("{crate_name}-rtmr2.hex"));
			fs::write(&rtmr2_path, &hex).unwrap();
			println!("cargo:warning=RTMR[2] written to: {}", rtmr2_path.display());
		}

		// ---------------------------------------------------------------
		// 10c. Precompute RTMR[1] (kernel boot measurement)
		// ---------------------------------------------------------------
		if kernel_output.exists() && final_path.exists() {
			eprintln!("==> Precomputing RTMR[1]...");
			let kernel_data = fs::read(&kernel_output).unwrap();
			let initrd_data = fs::read(&final_path).unwrap();
			rtmr1_value =
				mrtd::compute_rtmr1(&kernel_data, &initrd_data, &self.default_memory);
			let hex = rtmr1_value
				.iter()
				.map(|b| format!("{b:02x}"))
				.collect::<String>();
			println!("cargo:warning=RTMR[1]: {hex}");
			println!("cargo:rustc-env=TDX_EXPECTED_RTMR1={hex}");

			let rtmr1_path = target_dir
				.join(profile)
				.join(format!("{crate_name}-rtmr1.hex"));
			fs::write(&rtmr1_path, &hex).unwrap();
			println!("cargo:warning=RTMR[1] written to: {}", rtmr1_path.display(),);
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

		let bundle_path = target_dir
			.join(profile)
			.join(format!("{crate_name}-run-qemu.sh"));

		Some(BuiltOutput {
			initramfs_path: final_path,
			kernel_path: kernel_output,
			ovmf_path: ovmf_output,
			bundle_path,
			mrtd: Measurement::from(mrtd_value),
			rtmr1: Measurement::from(rtmr1_value),
			rtmr2: Measurement::from(rtmr2_value),
		})
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

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct BuiltOutput {
	pub initramfs_path: PathBuf,
	pub kernel_path: PathBuf,
	pub ovmf_path: PathBuf,
	pub bundle_path: PathBuf,
	pub mrtd: Measurement,
	pub rtmr1: Measurement,
	pub rtmr2: Measurement,
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
		self.write_entry(path, data, if executable { 0o100_755 } else { 0o100_644 })
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
		// Reject obviously truncated files (e.g. interrupted downloads).
		let size = fs::metadata(&cached).map(|m| m.len()).unwrap_or(0);
		if size > 1024 {
			eprintln!("  [cached] {}", cached.display());
			return cached;
		}
		eprintln!(
			"  [stale] {} is only {size} bytes — re-downloading",
			cached.display()
		);
		let _ = fs::remove_file(&cached);
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

/// Extract a `data.tar.*` archive into `target_dir`, handling `.zst`
/// compression explicitly (macOS and some Linux `tar` builds lack native
/// zstd support).
fn extract_data_tar(data_tar: &Path, target_dir: &Path) {
	let name = data_tar.file_name().unwrap().to_str().unwrap_or_default();

	if std::path::Path::new(name)
		.extension()
		.is_some_and(|ext| ext.eq_ignore_ascii_case("zst"))
	{
		let mut zstd = Command::new("zstd")
			.args(["-d", "-c"])
			.arg(data_tar)
			.stdout(Stdio::piped())
			.spawn()
			.expect("failed to run zstd — install zstd to extract .deb packages");

		let status = Command::new("tar")
			.args(["xf", "-", "-C"])
			.arg(target_dir)
			.stdin(zstd.stdout.take().unwrap())
			.status()
			.expect("failed to run tar");

		let zstd_status = zstd.wait().expect("zstd process failed");

		assert!(
			zstd_status.success() && status.success(),
			"failed to extract {} (zstd | tar)",
			data_tar.display()
		);
	} else if std::path::Path::new(name)
		.extension()
		.is_some_and(|ext| ext.eq_ignore_ascii_case("xz"))
		|| std::path::Path::new(name)
			.extension()
			.is_some_and(|ext| ext.eq_ignore_ascii_case("lzma"))
	{
		let mut xz = Command::new("xz")
			.args(["-d", "-c"])
			.arg(data_tar)
			.stdout(Stdio::piped())
			.spawn()
			.expect("failed to run xz — install xz to extract .deb packages");

		let status = Command::new("tar")
			.args(["xf", "-", "-C"])
			.arg(target_dir)
			.stdin(xz.stdout.take().unwrap())
			.status()
			.expect("failed to run tar");

		let xz_status = xz.wait().expect("xz process failed");
		assert!(
			xz_status.success() && status.success(),
			"failed to extract {} (xz | tar)",
			data_tar.display()
		);
	} else {
		// .gz / plain tar — handled natively by all tar implementations
		let status = Command::new("tar")
			.args(["xf"])
			.arg(data_tar)
			.args(["-C"])
			.arg(target_dir)
			.status()
			.expect("failed to run tar");
		assert!(status.success(), "failed to extract {}", data_tar.display());
	}
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

	// Reproducible builds: disable incremental compilation to
	// ensure identical output for identical source code.
	cmd.env("CARGO_INCREMENTAL", "0");

	// Reproducible builds: remap absolute paths out of the binary.
	//
	// --remap-path-prefix strips the local checkout and cargo
	// registry paths from panic messages, debug info, `file!()`,
	// and DWARF DW_AT_comp_dir entries.  This prevents the binary
	// from changing when the same source is built from a different
	// directory.
	//
	// We also set CARGO_ENCODED_RUSTFLAGS rather than RUSTFLAGS
	// because the former is the delimiter-safe cargo-internal
	// form and won't clobber any flags the user already set.
	let remap_flags = {
		let mut flags = String::new();

		// Remap the crate source tree
		flags.push_str(&format!(
			"--remap-path-prefix={}=/build",
			manifest_dir.display()
		));

		// Remap the cargo registry / git sources.
		// Always remap both CARGO_HOME (if set) and $HOME/.cargo
		// so the remap matches regardless of how cargo resolves it.
		let mut cargo_homes: Vec<String> = Vec::new();
		if let Ok(ch) = env::var("CARGO_HOME") {
			cargo_homes.push(ch);
		}
		if let Ok(home) = env::var("HOME") {
			let default = format!("{home}/.cargo");
			if !cargo_homes.contains(&default) {
				cargo_homes.push(default);
			}
		}
		for home in &cargo_homes {
			flags.push_str(&format!(
				"\x1f--remap-path-prefix={home}/registry/src=/registry"
			));
			flags.push_str(&format!(
				"\x1f--remap-path-prefix={home}/git/checkouts=/git"
			));
		}

		// Remap the rustup toolchain sysroot
		if let Ok(sysroot) = {
			Command::new("rustc")
				.args(["--print", "sysroot"])
				.output()
				.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
		} {
			if !sysroot.is_empty() {
				flags
					.push_str(&format!("\x1f--remap-path-prefix={sysroot}/lib=/rustlib"));
			}
		}

		flags
	};

	// Merge with any existing CARGO_ENCODED_RUSTFLAGS the user
	// already set so we don't silently clobber their flags.
	//
	// Also append linker + strip flags to the same variable:
	//  - --build-id=none: emit a zeroed ELF .note.gnu.build-id instead of one
	//    derived from file hashes (which vary with path-dependent debug info
	//    before stripping).
	//  - strip=symbols: remove all debug/symbol sections that carry absolute
	//    paths even after --remap-path-prefix.
	let existing = env::var("CARGO_ENCODED_RUSTFLAGS").unwrap_or_default();
	let mut combined = if existing.is_empty() {
		remap_flags
	} else {
		format!("{existing}\x1f{remap_flags}")
	};
	combined.push_str("\x1f-Clink-arg=-Wl,--build-id=none\x1f-Cstrip=symbols");
	cmd.env("CARGO_ENCODED_RUSTFLAGS", &combined);

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
		extract_data_tar(&data_tar, &extract_dir);
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
				extract_data_tar(&data_tar, &modules_extract_dir);
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
) -> u32 {
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

	let mut bundled_count: u32 = 0;

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
			bundled_count += 1;
		} else {
			eprintln!("  [module] {module_name}.ko not found (may be built-in)");
		}
	}

	bundled_count
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

#[allow(clippy::ref_option)]
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
		extract_data_tar(&data_tar, &ovmf_extract);
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
	let uncompressed_tar = out_dir.join("payload.tar");
	let mut tar_args = vec!["vmlinuz", "initramfs.cpio.gz"];
	if ovmf_output.exists() {
		tar_args.push("OVMF.fd");
	}

	// Reproducible tar: SOURCE_DATE_EPOCH=0 clamps file mtimes to
	// epoch, --numeric-owner avoids embedding user/group names.
	// Compression is handled separately by gzip -n to suppress the
	// OS byte and MTIME header field.
	let tar_status = Command::new("tar")
		.args(["cf"])
		.arg(&uncompressed_tar)
		.args(["--numeric-owner", "-C"])
		.arg(&bundle_dir)
		.args(&tar_args)
		.env("SOURCE_DATE_EPOCH", "0")
		.status()
		.expect("failed to run tar");

	if !tar_status.success() {
		eprintln!("  [warn] Failed to create tar payload — skipping run-qemu.sh");
		let _ = fs::remove_dir_all(&bundle_dir);
		let _ = fs::remove_file(&uncompressed_tar);
		return;
	}

	let gz_file = File::create(&tar_path).unwrap();
	let gz_status = Command::new("gzip")
		.args(["-9", "-n", "-c"])
		.arg(&uncompressed_tar)
		.stdout(gz_file)
		.status()
		.expect("failed to run gzip");

	if !gz_status.success() {
		eprintln!("  [warn] Failed to compress tar payload — skipping run-qemu.sh");
		let _ = fs::remove_dir_all(&bundle_dir);
		let _ = fs::remove_file(&uncompressed_tar);
		return;
	}
	let _ = fs::remove_file(&uncompressed_tar);

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
	use sha2::{Digest, Sha384};

	const PAGE_SIZE: usize = 4096;
	const CHUNK_SIZE: usize = 256;
	const CHUNKS_PER_PAGE: usize = PAGE_SIZE / CHUNK_SIZE;
	const SHA384_LEN: usize = 48;

	// e47a6535-984a-4798-865e-4685a7bf8ec2 (little-endian)
	const TDVF_METADATA_GUID: [u8; 16] = [
		0x35, 0x65, 0x7a, 0xe4, 0x4a, 0x98, 0x98, 0x47, 0x86, 0x5e, 0x46, 0x85,
		0xa7, 0xbf, 0x8e, 0xc2,
	];

	// 96b582de-1fb2-45f7-baea-a366c55a082d (little-endian)
	const TABLE_FOOTER_GUID: [u8; 16] = [
		0xde, 0x82, 0xb5, 0x96, 0xb2, 0x1f, 0xf7, 0x45, 0xba, 0xea, 0xa3, 0x66,
		0xc5, 0x5a, 0x08, 0x2d,
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

	/// Locate the start of the TDVF metadata descriptor in the
	/// OVMF image. Returns the byte offset (from the start of
	/// `ovmf`) where the descriptor begins.
	///
	/// Two discovery methods are tried in order:
	///  1. **GUID table** (modern OVMF builds): a footer GUID at `file_end −
	///     0x30` points to a table of entries; the TDVF metadata offset entry
	///     stores a negative offset from the end of the file.
	///  2. **Legacy pointer** (deprecated): a 32-bit *absolute* offset stored at
	///     `file_end − 0x20`.
	fn find_tdvf_descriptor_offset(ovmf: &[u8]) -> Result<usize, String> {
		let len = ovmf.len();

		// --- Method 1: GUID table ---
		if len >= 0x32 {
			let footer_start = len - 0x30;
			let footer_guid = &ovmf[footer_start..footer_start + 16];

			if footer_guid == TABLE_FOOTER_GUID {
				eprintln!("  [tdvf] Found GUID table footer");

				let table_size =
					u16::from_le_bytes(ovmf[len - 0x32..len - 0x30].try_into().unwrap())
						as usize;

				let table_start = len - 0x20 - table_size;
				let table = &ovmf[table_start..table_start + table_size];

				// Walk backward: skip footer GUID (16) + size (2)
				let mut offset = table_size.saturating_sub(18);
				while offset >= 18 {
					let entry_guid = &table[offset - 16..offset];
					let entry_size = u16::from_le_bytes(
						table[offset - 18..offset - 16].try_into().unwrap(),
					) as usize;

					if entry_size == 0 {
						break;
					}
					offset -= entry_size;

					if entry_guid == TDVF_METADATA_GUID && entry_size == 22 {
						let desc_off =
							u32::from_le_bytes(table[offset..offset + 4].try_into().unwrap())
								as usize;
						eprintln!("  [tdvf] GUID table: descriptor at end-0x{desc_off:x}");
						return Ok(len - desc_off);
					}
				}
				eprintln!(
					"  [tdvf] GUID table present but TDVF metadata entry not found, \
					 trying legacy"
				);
			}
		}

		// --- Method 2: legacy absolute offset at file_end - 0x20 ---
		if len >= 0x28 {
			let descriptor_offset =
				u32::from_le_bytes(ovmf[len - 0x20..len - 0x1c].try_into().unwrap())
					as usize;

			if descriptor_offset > 0 && descriptor_offset < len {
				eprintln!(
					"  [tdvf] Legacy pointer: descriptor at 0x{descriptor_offset:x}"
				);
				return Ok(descriptor_offset);
			}
		}

		Err(format!(
			"Could not locate TDVF metadata (no GUID table entry, legacy offset is \
			 0x0, file size: 0x{len:x})"
		))
	}

	const TDVF_SIGNATURE: &[u8; 4] = b"TDVF";
	const TDVF_HEADER_SIZE: usize = 16; // signature(4) + length(4) + version(4) + num_sections(4)

	fn parse_tdvf_sections(ovmf: &[u8]) -> Result<Vec<TdvfSection>, String> {
		let desc_start = find_tdvf_descriptor_offset(ovmf)?;
		let desc = &ovmf[desc_start..];

		if desc.len() < TDVF_HEADER_SIZE {
			return Err("TDVF descriptor region too small".into());
		}

		let signature = &desc[0..4];
		if signature != TDVF_SIGNATURE {
			return Err(format!(
				"TDVF descriptor signature mismatch: expected \"TDVF\", got \
				 {signature:02x?}"
			));
		}

		let _length = u32::from_le_bytes(desc[4..8].try_into().unwrap());
		let version = u32::from_le_bytes(desc[8..12].try_into().unwrap());
		let num_sections = u32::from_le_bytes(desc[12..16].try_into().unwrap());

		eprintln!("  [tdvf] version={version}, sections={num_sections}");

		let mut sections = Vec::new();
		for i in 0..num_sections as usize {
			let base = TDVF_HEADER_SIZE + i * 32;
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

	pub fn compute_mrtd(ovmf: &[u8]) -> Result<[u8; SHA384_LEN], String> {
		let sections = parse_tdvf_sections(ovmf)?;

		// The TDX module maintains a running SHA-384 context across
		// all TDH.MEM.PAGE.ADD and TDH.MR.EXTEND calls, then
		// finalizes it on TDH.MR.FINALIZE.  This is equivalent to
		// SHA384(concat of all 128-byte headers and data chunks).
		let mut hasher = Sha384::new();

		for section in &sections {
			let extend_mr = section.attributes & ATTR_MR_EXTEND != 0;
			let page_aug = section.attributes & ATTR_PAGE_AUG != 0;

			// PAGE_AUG pages are added lazily via TDH.MEM.PAGE.AUG
			// which does NOT update MRTD.
			if page_aug {
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
				"  [mrtd] section GPA 0x{gpa_base:x}, {num_pages} pages{}",
				if extend_mr { " [MR.EXTEND]" } else { "" },
			);

			// Phase 1: TDH.MEM.PAGE.ADD for every page in this section
			for page_idx in 0..num_pages {
				let page_gpa = gpa_base + (page_idx as u64) * (PAGE_SIZE as u64);

				let mut add_buf = [0u8; 128];
				add_buf[..12].copy_from_slice(b"MEM.PAGE.ADD");
				add_buf[16..24].copy_from_slice(&page_gpa.to_le_bytes());

				hasher.update(&add_buf);
			}

			// Phase 2: TDH.MR.EXTEND for every 256-byte chunk (only
			// for sections with the MR_EXTEND attribute)
			if extend_mr {
				for page_idx in 0..num_pages {
					let page_gpa = gpa_base + (page_idx as u64) * (PAGE_SIZE as u64);

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

						hasher.update(&ext_header);
						hasher.update(chunk_data);
					}
				}
			}
		}

		let mut mrtd = [0u8; SHA384_LEN];
		mrtd.copy_from_slice(&hasher.finalize());

		let hex = mrtd.iter().map(|b| format!("{b:02x}")).collect::<String>();
		eprintln!("  [mrtd] MRTD = {hex}");

		Ok(mrtd)
	}

	/// Compute the expected RTMR[2] register value.
	///
	/// RTMR[2] receives exactly two events during TDX boot:
	///  1. Kernel command line (UTF-16LE with null terminator)
	///  2. Initrd (raw file bytes, typically gzipped CPIO)
	///
	/// Each event extends the register:
	///   `RTMR[2] = SHA384(RTMR[2] || SHA384(preimage))`
	///
	/// For non-UKI boots (separate -kernel/-initrd), QEMU appends
	/// `" initrd=initrd"` to the measured command line when an initrd
	/// is present.
	pub fn compute_rtmr2(cmdline: &str, initrd: &[u8]) -> [u8; SHA384_LEN] {
		let mut rtmr2 = [0u8; SHA384_LEN];

		// Event 1: kernel command line
		// QEMU appends " initrd=initrd" when an initrd is provided.
		let measured_cmdline = if !initrd.is_empty() {
			format!("{cmdline} initrd=initrd\0")
		} else {
			format!("{cmdline}\0")
		};

		// Encode as UTF-16LE
		let cmdline_utf16: Vec<u8> = measured_cmdline
			.encode_utf16()
			.flat_map(|c| c.to_le_bytes())
			.collect();

		let cmdline_digest = Sha384::digest(&cmdline_utf16);
		let mut extend_input = Vec::with_capacity(SHA384_LEN * 2);
		extend_input.extend_from_slice(&rtmr2);
		extend_input.extend_from_slice(&cmdline_digest);
		rtmr2.copy_from_slice(&Sha384::digest(&extend_input));

		eprintln!(
			"  [rtmr2] event 1: cmdline ({} UTF-16LE bytes)",
			cmdline_utf16.len(),
		);

		// Event 2: initrd (raw file bytes as loaded by QEMU)
		if !initrd.is_empty() {
			let initrd_digest = Sha384::digest(initrd);
			extend_input.clear();
			extend_input.extend_from_slice(&rtmr2);
			extend_input.extend_from_slice(&initrd_digest);
			rtmr2.copy_from_slice(&Sha384::digest(&extend_input));

			eprintln!(
				"  [rtmr2] event 2: initrd ({:.1} MB)",
				initrd.len() as f64 / 1_048_576.0,
			);
		}

		let hex = rtmr2.iter().map(|b| format!("{b:02x}")).collect::<String>();
		eprintln!("  [rtmr2] RTMR[2] = {hex}");

		rtmr2
	}

	// -----------------------------------------------------------------
	// RTMR[1] — kernel boot measurement
	// -----------------------------------------------------------------

	/// QEMU memory layout: determines the below-4G RAM size which
	/// affects where the initrd is loaded and thus the kernel setup
	/// header patches.
	struct MemoryLayout {
		below_4g: u64,
	}

	/// Size of QEMU's ACPI data region.
	const ACPI_DATA_SIZE: u64 = 0x20000 + 0x8000;

	/// Parse a QEMU `-m` memory string into bytes.
	/// Accepts "4G", "4096M", or bare MiB number.
	fn parse_memory_bytes(s: &str) -> u64 {
		let s = s.trim();
		if let Some(n) = s.strip_suffix('G').or_else(|| s.strip_suffix('g')) {
			n.trim().parse::<u64>().unwrap() * 1024 * 1024 * 1024
		} else if let Some(n) = s.strip_suffix('M').or_else(|| s.strip_suffix('m'))
		{
			n.trim().parse::<u64>().unwrap() * 1024 * 1024
		} else {
			s.parse::<u64>().unwrap() * 1024 * 1024
		}
	}

	/// Compute the QEMU PC memory layout from total RAM bytes.
	fn memory_layout(total: u64) -> MemoryLayout {
		let lowmem = if total >= 0xb000_0000 {
			0x8000_0000
		} else {
			0xb000_0000
		};
		MemoryLayout {
			below_4g: if total >= lowmem { lowmem } else { total },
		}
	}

	/// Replicate QEMU's `x86_load_linux` kernel setup-header patches.
	///
	/// QEMU modifies the kernel image in memory before OVMF measures
	/// it, so the PE Authenticode hash must be computed over the
	/// patched bytes.
	fn patch_kernel_setup(
		kernel: &mut [u8],
		initrd_len: usize,
		layout: &MemoryLayout,
	) {
		let magic = &kernel[0x202..0x206];
		let protocol = if magic == b"HdrS" {
			u16::from_le_bytes(kernel[0x206..0x208].try_into().unwrap())
		} else {
			0
		};

		let (real_addr, cmdline_addr): (u32, u32) =
			if protocol >= 0x202 && (kernel[0x211] & 0x01) != 0 {
				(0x10000, 0x20000)
			} else {
				(0x90000, 0x9a000_u32.wrapping_sub(32))
			};

		// Determine maximum initrd address
		let mut initrd_max: u64 = if protocol >= 0x20c
			&& (u16::from_le_bytes(kernel[0x236..0x238].try_into().unwrap()) & 2) != 0
		{
			0xffff_ffff
		} else if protocol >= 0x203 {
			u32::from_le_bytes(kernel[0x22c..0x230].try_into().unwrap()) as u64
		} else {
			0x37ff_ffff
		};

		let mem_cap = layout.below_4g - ACPI_DATA_SIZE;
		if initrd_max >= mem_cap {
			initrd_max = mem_cap - 1;
		}

		// Patch cmdline pointer
		if protocol >= 0x202 {
			kernel[0x228..0x22c].copy_from_slice(&cmdline_addr.to_le_bytes());
		} else {
			kernel[0x20..0x22].copy_from_slice(&0xa33f_u16.to_le_bytes());
			kernel[0x22..0x24].copy_from_slice(
				&(cmdline_addr.wrapping_sub(real_addr) as u16).to_le_bytes(),
			);
		}

		// type_of_loader
		if protocol >= 0x200 {
			kernel[0x210] = 0xb0;
		}

		// loadflags + heap_end_ptr
		if protocol >= 0x201 {
			kernel[0x211] |= 0x80;
			kernel[0x224..0x226].copy_from_slice(
				&((cmdline_addr.wrapping_sub(real_addr).wrapping_sub(0x200)) as u16)
					.to_le_bytes(),
			);
		}

		// initrd address + size
		if initrd_len > 0 {
			assert!(
				(initrd_len as u64) < initrd_max,
				"initrd too large for memory layout"
			);
			let initrd_addr = ((initrd_max - initrd_len as u64) & !4095) as u32;
			kernel[0x218..0x21c].copy_from_slice(&initrd_addr.to_le_bytes());
			kernel[0x21c..0x220].copy_from_slice(&(initrd_len as u32).to_le_bytes());
		}
	}

	/// Compute the PE Authenticode hash preimage.
	///
	/// This concatenates the PE file contents in hash order: headers
	/// (excluding the checksum field and certificate directory entry),
	/// then section bodies sorted by raw data pointer, then any
	/// trailing data minus the certificate blob.
	fn pe_hash_preimage(data: &[u8]) -> Vec<u8> {
		let e_lfanew =
			u32::from_le_bytes(data[0x3c..0x40].try_into().unwrap()) as usize;

		let file_header_offset = e_lfanew + 4;
		let num_sections = u16::from_le_bytes(
			data[file_header_offset + 2..file_header_offset + 4]
				.try_into()
				.unwrap(),
		) as usize;
		let size_of_optional_header = u16::from_le_bytes(
			data[file_header_offset + 16..file_header_offset + 18]
				.try_into()
				.unwrap(),
		) as usize;
		let opt = file_header_offset + 20;

		let magic = u16::from_le_bytes(data[opt..opt + 2].try_into().unwrap());
		let fixed_opt_size: usize = match magic {
			0x10b => 96,  // PE32
			0x20b => 112, // PE32+
			_ => panic!("Unknown PE optional-header magic: {magic:#x}"),
		};

		let size_of_headers =
			u32::from_le_bytes(data[opt + 60..opt + 64].try_into().unwrap()) as usize;
		let num_rva = u32::from_le_bytes(
			data[opt + fixed_opt_size - 4..opt + fixed_opt_size]
				.try_into()
				.unwrap(),
		) as usize;

		let checksum_off = opt + 0x40;
		let security_dir_idx = 4;

		let mut parts: Vec<&[u8]> = Vec::new();

		// Everything before the checksum field
		parts.push(&data[..checksum_off]);

		// After checksum, skip cert-dir entry if present
		let after_checksum = checksum_off + 4;
		if num_rva <= security_dir_idx {
			parts.push(&data[after_checksum..size_of_headers]);
		} else {
			let cert_dir_off = opt + fixed_opt_size + security_dir_idx * 8;
			parts.push(&data[after_checksum..cert_dir_off]);
			let after_cert = cert_dir_off + 8;
			parts.push(&data[after_cert..size_of_headers]);
		}

		// Section bodies sorted by raw-data pointer
		struct SecInfo {
			ptr: usize,
			size: usize,
		}

		let sh_start = opt + size_of_optional_header;
		let mut sections: Vec<SecInfo> = (0..num_sections)
			.filter_map(|i| {
				let sh = sh_start + i * 40;
				let raw_size =
					u32::from_le_bytes(data[sh + 16..sh + 20].try_into().unwrap())
						as usize;
				let raw_ptr =
					u32::from_le_bytes(data[sh + 20..sh + 24].try_into().unwrap())
						as usize;
				if raw_size > 0 {
					Some(SecInfo {
						ptr: raw_ptr,
						size: raw_size,
					})
				} else {
					None
				}
			})
			.collect();
		sections.sort_by_key(|s| s.ptr);

		let mut sum_hashed = size_of_headers;
		for s in &sections {
			parts.push(&data[s.ptr..s.ptr + s.size]);
			sum_hashed += s.size;
		}

		// Trailing data beyond sections, minus cert blob
		if data.len() > sum_hashed {
			let mut cert_size = 0usize;
			if num_rva > security_dir_idx {
				let off = opt + fixed_opt_size + security_dir_idx * 8 + 4;
				if off + 4 <= data.len() {
					cert_size =
						u32::from_le_bytes(data[off..off + 4].try_into().unwrap()) as usize;
				}
			}
			if data.len() > sum_hashed + cert_size {
				parts.push(&data[sum_hashed..data.len() - cert_size]);
			}
		}

		let total_len: usize = parts.iter().map(|p| p.len()).sum();
		let mut result = Vec::with_capacity(total_len);
		for part in parts {
			result.extend_from_slice(part);
		}
		result
	}

	/// Precompute RTMR[1] for a non-UKI TDX boot.
	///
	/// RTMR[1] receives four events:
	///  1. Kernel PE Authenticode hash (after QEMU setup-header patches)
	///  2. "Calling EFI Application from Boot Option"
	///  3. "Exit Boot Services Invocation"
	///  4. "Exit Boot Services Returned with Success"
	///
	/// The kernel is cloned and patched exactly as QEMU does before
	/// OVMF measures it: cmdline pointer, initrd address/size,
	/// type_of_loader, loadflags, and heap_end_ptr.
	pub fn compute_rtmr1(
		kernel: &[u8],
		initrd: &[u8],
		memory: &str,
	) -> [u8; SHA384_LEN] {
		let mut rtmr1 = [0u8; SHA384_LEN];
		let mut extend = Vec::with_capacity(SHA384_LEN * 2);

		// Clone kernel and apply QEMU setup-header patches
		let mut patched = kernel.to_vec();
		let layout = memory_layout(parse_memory_bytes(memory));
		patch_kernel_setup(&mut patched, initrd.len(), &layout);

		// Event 1: kernel PE Authenticode hash
		let preimage = pe_hash_preimage(&patched);
		let digest = Sha384::digest(&preimage);
		extend.extend_from_slice(&rtmr1);
		extend.extend_from_slice(&digest);
		rtmr1.copy_from_slice(&Sha384::digest(&extend));
		eprintln!(
			"  [rtmr1] event 1: kernel PE hash ({} bytes preimage)",
			preimage.len(),
		);

		// Events 2-4: fixed EFI action strings
		const EFI_ACTIONS: [&[u8]; 3] = [
			b"Calling EFI Application from Boot Option",
			b"Exit Boot Services Invocation",
			b"Exit Boot Services Returned with Success",
		];

		for (i, action) in EFI_ACTIONS.iter().enumerate() {
			let digest = Sha384::digest(action);
			extend.clear();
			extend.extend_from_slice(&rtmr1);
			extend.extend_from_slice(&digest);
			rtmr1.copy_from_slice(&Sha384::digest(&extend));
			eprintln!(
				"  [rtmr1] event {}: {:?}",
				i + 2,
				std::str::from_utf8(action).unwrap(),
			);
		}

		let hex = rtmr1.iter().map(|b| format!("{b:02x}")).collect::<String>();
		eprintln!("  [rtmr1] RTMR[1] = {hex}");

		rtmr1
	}
}
