//! `AlpineBuilder` — fluent API for assembling TDX guest images.

use {
	super::{
		super::{
			cpio::CpioWriter,
			helpers::{
				detect_profile,
				download_cached,
				env_or,
				extract_file_from_tar_gz,
				find_target_dir,
				list_tar_gz,
			},
			kernel::{auto_download_kernel, bundle_auto_discovered_modules},
			mrtd,
			ovmf::obtain_ovmf,
			scripts::{generate_launch_script, generate_self_extracting_script},
		},
		cross::cross_compile_musl,
	},
	crate::tee::tdx::Measurement,
	std::{
		env,
		fs::{self, File},
		io::BufWriter,
		path::{Path, PathBuf},
		process::Command,
	},
};

/// Builder for Alpine-based TDX guest images.
///
/// Constructed via [`mosaik::tee::tdx::build::alpine()`].
pub struct AlpineBuilder {
	pub(super) alpine_major: Option<String>,
	pub(super) alpine_minor: Option<String>,
	pub(super) custom_minirootfs: Option<Vec<u8>>,
	pub(super) custom_vmlinuz: Option<Vec<u8>>,
	pub(super) custom_ovmf: Option<Vec<u8>>,
	pub(super) ssh_keys: Vec<String>,
	pub(super) ssh_forward: Option<(u16, u16)>,
	pub(super) default_cpus: u32,
	pub(super) default_memory: String,
	pub(super) bundle_runner: bool,
	pub(super) extra_files: Vec<(PathBuf, String)>,
	pub(super) extra_kernel_modules: Vec<PathBuf>,
	pub(super) kernel_modules_dir: Option<PathBuf>,
	pub(super) kernel_version: Option<String>,
	pub(super) kernel_abi: Option<String>,
	pub(super) ovmf_version: Option<String>,
	pub(super) artifacts_output_path: Option<PathBuf>,
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

	/// Provide a custom ovmf.fd firmware (in-memory bytes).
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
	/// Use [`with_ssh_key`](Self::with_ssh_key) to authorize public
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
	/// Can be called multiple times to authorize several keys.
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

	/// Set a custom path for build artifacts.
	///
	/// If `path` is absolute it is used as-is. If it is relative it is
	/// resolved against the Cargo target directory.
	///
	/// Defaults to `<target_dir>/tdx-artifacts/alpine`.
	#[must_use]
	pub fn with_artifacts_output_path(
		mut self,
		path: impl Into<PathBuf>,
	) -> Self {
		self.artifacts_output_path = Some(path.into());
		self
	}

	/// Build the TDX guest image.
	///
	/// # Panics
	///
	/// Panics if any required tool (`curl`, `tar`, `gzip`, `ar`,
	/// etc.) is missing or if cross-compilation fails.
	#[allow(clippy::too_many_lines)]
	pub fn build(self) -> Option<super::super::BuilderOutput> {
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

		// Resolve artifacts output directory
		let artifacts_dir = match self.artifacts_output_path {
			Some(ref p) if p.is_absolute() => p.clone(),
			Some(ref p) => target_dir.join(p),
			None => target_dir
				.join(profile)
				.join("tdx-artifacts")
				.join(&crate_name)
				.join("alpine"),
		};
		fs::create_dir_all(&artifacts_dir).unwrap();

		// Resolve Alpine version: builder field > env var > default
		let alpine_ver = self
			.alpine_major
			.unwrap_or_else(|| env_or("TDX_IMAGE_ALPINE_VERSION", "3.21"));
		let alpine_minor = self
			.alpine_minor
			.unwrap_or_else(|| env_or("TDX_IMAGE_ALPINE_MINOR", "0"));

		let output_filename = format!("{crate_name}-initramfs.cpio.gz");
		let final_path = artifacts_dir.join(&output_filename);

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

		let kernel_vmlinuz: Option<PathBuf>;
		let mut auto_modules_dir: Option<PathBuf> = None;

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
		let kernel_output = artifacts_dir.join(format!("{crate_name}-vmlinuz"));

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
			&artifacts_dir,
			self.default_cpus,
			&self.default_memory,
			self.ssh_forward,
		);

		// ---------------------------------------------------------------
		// 10. Obtain ovmf.fd and precompute MRTD
		// ---------------------------------------------------------------
		let ovmf_output = artifacts_dir.join(format!("{crate_name}-ovmf.fd"));

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
					let hex = hex::encode(digest);
					println!("cargo:warning=MRTD: {hex}");
					println!("cargo:rustc-env=TDX_EXPECTED_MRTD={hex}");

					let mrtd_path = artifacts_dir.join(format!("{crate_name}-mrtd.hex"));
					fs::write(&mrtd_path, &hex).unwrap();
					println!("cargo:warning=MRTD written to: {}", mrtd_path.display());
				}
				Err(e) => {
					println!("cargo:warning=MRTD computation failed: {e}");
				}
			}
		}

		// ---------------------------------------------------------------
		// 10b. Precompute RTMR[1] (kernel boot measurement)
		// ---------------------------------------------------------------
		if kernel_output.exists() && final_path.exists() {
			eprintln!("==> Precomputing RTMR[1]...");
			let kernel_data = fs::read(&kernel_output).unwrap();
			let initrd_data = fs::read(&final_path).unwrap();
			rtmr1_value =
				mrtd::compute_rtmr1(&kernel_data, &initrd_data, &self.default_memory);
			let hex = hex::encode(rtmr1_value);
			println!("cargo:warning=RTMR[1]: {hex}");
			println!("cargo:rustc-env=TDX_EXPECTED_RTMR1={hex}");

			let rtmr1_path = artifacts_dir.join(format!("{crate_name}-rtmr1.hex"));
			fs::write(&rtmr1_path, &hex).unwrap();
			println!("cargo:warning=RTMR[1] written to: {}", rtmr1_path.display(),);
		}

		// ---------------------------------------------------------------
		// 10c. Precompute RTMR[2] (initrd + cmdline measurement)
		// ---------------------------------------------------------------
		if final_path.exists() {
			eprintln!("==> Precomputing RTMR[2]...");
			let initrd_data = fs::read(&final_path).unwrap();
			let cmdline = "console=ttyS0 ip=dhcp";
			rtmr2_value = mrtd::compute_rtmr2(cmdline, &initrd_data);
			let hex = hex::encode(rtmr2_value);
			println!("cargo:warning=RTMR[2]: {hex}");
			println!("cargo:rustc-env=TDX_EXPECTED_RTMR2={hex}");

			let rtmr2_path = artifacts_dir.join(format!("{crate_name}-rtmr2.hex"));
			fs::write(&rtmr2_path, &hex).unwrap();
			println!("cargo:warning=RTMR[2] written to: {}", rtmr2_path.display());
		}

		// ---------------------------------------------------------------
		// 11. Generate self-extracting run-qemu.sh
		// ---------------------------------------------------------------
		generate_self_extracting_script(
			&crate_name,
			&artifacts_dir,
			&out_dir,
			&kernel_output,
			&final_path,
			&ovmf_output,
			self.default_cpus,
			&self.default_memory,
			self.ssh_forward,
		);

		let bundle_path = artifacts_dir.join(format!("{crate_name}-run-qemu.sh"));

		Some(super::super::BuilderOutput {
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
			artifacts_output_path: None,
		}
	}
}

const UDHCPC_SCRIPT: &str = include_str!("templates/udhcpc.sh");

fn generate_init_script(crate_name: &str, ssh_keys: &[String]) -> String {
	let ssh_block = if ssh_keys.is_empty() {
		String::new()
	} else {
		include_str!("../templates/init-ssh.sh")
			.replace("{{SSH_KEYS}}", &ssh_keys.join("\n"))
	};

	include_str!("templates/init.sh")
		.replace("{{CRATE_NAME}}", crate_name)
		.replace("{{SSH_BLOCK}}", &ssh_block)
}
