//! `UbuntuBuilder` — fluent API for assembling glibc-based TDX guest
//! images.

use {
	super::{
		super::{
			helpers::{
				detect_profile,
				download_cached,
				env_or,
				extract_data_tar,
				find_file_starting_with,
				find_target_dir,
			},
			kernel::auto_download_kernel,
			mrtd,
			ovmf::obtain_ovmf,
			scripts::{generate_launch_script, generate_self_extracting_script},
		},
		cross::cross_compile_gnu,
	},
	crate::tee::tdx::Measurement,
	std::{
		env,
		fs::{self, File},
		os::unix::fs::PermissionsExt,
		path::{Path, PathBuf},
		process::Command,
	},
};

/// Builder for Ubuntu/glibc-based TDX guest images.
///
/// Constructed via [`mosaik::tee::tdx::build::ubuntu()`].
///
/// The application binary is cross-compiled for
/// `x86_64-unknown-linux-gnu` (glibc). The init environment is
/// based on the Ubuntu minimal rootfs which provides glibc, a
/// shell, and standard utilities.
pub struct UbuntuBuilder {
	pub(super) ubuntu_version: Option<String>,
	pub(super) custom_rootfs: Option<Vec<u8>>,
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

impl UbuntuBuilder {
	/// Set the Ubuntu base version (e.g. `"24.04.4"`).
	/// Defaults to `"24.04.4"` or `TDX_IMAGE_UBUNTU_VERSION` env
	/// var.
	#[must_use]
	pub fn with_ubuntu_version(mut self, version: &str) -> Self {
		self.ubuntu_version = Some(version.to_string());
		self
	}

	/// Provide a custom Ubuntu base rootfs tarball (in-memory
	/// bytes). When set, skips downloading from Ubuntu CDN.
	#[must_use]
	pub fn with_custom_rootfs(mut self, bytes: &[u8]) -> Self {
		self.custom_rootfs = Some(bytes.to_vec());
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

	/// Enable SSH access in the guest by forwarding a host port to
	/// the guest SSH daemon.
	///
	/// `host_port` is the port on the host, `guest_port` is the
	/// port inside the VM (typically 22). Calling this also adds the
	/// `hostfwd` rule to the generated QEMU launch scripts.
	///
	/// Use [`with_ssh_key`](Self::with_ssh_key) to authorize public
	/// keys.
	#[must_use]
	pub const fn with_ssh_forward(
		mut self,
		host_port: u16,
		guest_port: u16,
	) -> Self {
		self.ssh_forward = Some((host_port, guest_port));
		self
	}

	/// Add an SSH public key to the guest's
	/// `/root/.ssh/authorized_keys`.
	///
	/// Can be called multiple times to authorize several keys.
	/// Has no effect unless
	/// [`with_ssh_forward`](Self::with_ssh_forward) is also called.
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
	/// `host_path` is the path on the build host, `guest_path` is
	/// the absolute path inside the guest (leading `/` is stripped).
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
	/// If `path` is absolute it is used as-is. If it is relative it
	/// is resolved against the Cargo target directory.
	///
	/// Defaults to `<target_dir>/tdx-artifacts/ubuntu`.
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
		// --- Recursion guard -----------------------------------
		if env::var("__TDX_IMAGE_INNER_BUILD").is_ok() {
			return None;
		}

		println!("cargo:rerun-if-env-changed=TDX_IMAGE_SKIP");
		println!("cargo:rerun-if-env-changed=TDX_IMAGE_UBUNTU_VERSION");
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
				.join("ubuntu"),
		};
		fs::create_dir_all(&artifacts_dir).unwrap();

		let output_filename = format!("{crate_name}-initramfs.cpio.gz");
		let final_path = artifacts_dir.join(&output_filename);

		eprintln!(
			"==> TDX initramfs (ubuntu): profile={profile}, crate={crate_name}"
		);
		eprintln!("    output = {}", final_path.display());

		// ---------------------------------------------------
		// 1. Cross-compile for x86_64-unknown-linux-gnu
		// ---------------------------------------------------
		let binary_data = cross_compile_gnu(&crate_name, profile, &target_dir);

		// ---------------------------------------------------
		// 2. Download / use Ubuntu minimal rootfs
		// ---------------------------------------------------
		println!("cargo:rerun-if-env-changed=TDX_IMAGE_UBUNTU_VERSION");

		let ubuntu_ver = self
			.ubuntu_version
			.unwrap_or_else(|| env_or("TDX_IMAGE_UBUNTU_VERSION", "24.04.4"));

		// The CDN path uses the major version (e.g. "24.04") while
		// the tarball name contains the full point release.
		let ubuntu_major = ubuntu_ver
			.match_indices('.')
			.nth(1)
			.map_or(ubuntu_ver.as_str(), |(i, _)| &ubuntu_ver[..i]);

		let ubuntu_tar_path = self.custom_rootfs.as_ref().map_or_else(
			|| {
				eprintln!("==> Downloading Ubuntu minimal rootfs {ubuntu_ver}...");
				let ubuntu_tar_name =
					format!("ubuntu-base-{ubuntu_ver}-base-amd64.tar.gz");
				let ubuntu_url = format!(
					"https://cdimage.ubuntu.com/ubuntu-base/\
					 releases/{ubuntu_major}/release/{ubuntu_tar_name}"
				);
				download_cached(&ubuntu_url, &cache_dir, &ubuntu_tar_name)
			},
			|custom_rootfs| {
				let path = cache_dir.join("custom-rootfs.tar.gz");
				fs::write(&path, custom_rootfs).unwrap();
				path
			},
		);

		// ---------------------------------------------------
		// 3. Extract Ubuntu rootfs to working directory
		// ---------------------------------------------------
		eprintln!("==> Extracting Ubuntu rootfs...");

		let rootfs_dir = out_dir.join("ubuntu-rootfs");
		let _ = fs::remove_dir_all(&rootfs_dir);
		fs::create_dir_all(&rootfs_dir).unwrap();

		let tar_status = Command::new("tar")
			.args(["xzf"])
			.arg(&ubuntu_tar_path)
			.args(["-C"])
			.arg(&rootfs_dir)
			.status()
			.expect("failed to run tar");
		assert!(tar_status.success(), "Failed to extract Ubuntu rootfs");

		// ---------------------------------------------------
		// 3b. Install required packages missing from minbase
		// ---------------------------------------------------
		install_packages(&cache_dir, &rootfs_dir);

		// ---------------------------------------------------
		// 4. Generate /init script and overlay custom files
		// ---------------------------------------------------
		eprintln!("==> Generating /init...");
		let init_script = generate_ubuntu_init_script(&crate_name, &self.ssh_keys);
		fs::write(rootfs_dir.join("init"), &init_script).unwrap();
		set_executable(&rootfs_dir.join("init"));

		// Ensure standard directories exist
		for dir in &["lib/modules", "sys/kernel/config", "dev/pts", "dev/shm"] {
			fs::create_dir_all(rootfs_dir.join(dir)).unwrap();
		}

		// Place the application binary
		fs::create_dir_all(rootfs_dir.join("usr/bin")).unwrap();
		fs::write(
			rootfs_dir.join(format!("usr/bin/{crate_name}")),
			&binary_data,
		)
		.unwrap();
		set_executable(&rootfs_dir.join(format!("usr/bin/{crate_name}")));

		// ---------------------------------------------------
		// 5. Obtain TDX-enabled kernel + modules
		// ---------------------------------------------------
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

		// ---------------------------------------------------
		// 6. Stage extra files into rootfs, then create CPIO
		// ---------------------------------------------------

		// Extra files from builder API
		for (host_path, guest_path) in &self.extra_files {
			eprintln!("  [extra] {} → /{guest_path}", host_path.display());
			println!("cargo:rerun-if-changed={}", host_path.display());
			let dest = rootfs_dir.join(guest_path);
			if let Some(parent) = dest.parent() {
				fs::create_dir_all(parent).unwrap();
			}
			fs::copy(host_path, &dest).unwrap_or_else(|e| {
				panic!("Failed to copy {}: {e}", host_path.display())
			});
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
					let dest = rootfs_dir.join(guest_path);
					if let Some(parent) = dest.parent() {
						fs::create_dir_all(parent).unwrap();
					}
					fs::copy(host_path, &dest)
						.unwrap_or_else(|e| panic!("Failed to copy {host_path}: {e}"));
				}
			}
		}

		// Kernel modules from builder API
		for module_path in &self.extra_kernel_modules {
			let filename = module_path.file_name().unwrap().to_str().unwrap();
			let guest_path = format!("lib/modules/{filename}");
			eprintln!("  [module] {} → /{guest_path}", module_path.display());
			println!("cargo:rerun-if-changed={}", module_path.display());
			let dest = rootfs_dir.join(&guest_path);
			fs::copy(module_path, &dest).unwrap_or_else(|e| {
				panic!("Failed to copy {}: {e}", module_path.display())
			});
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
				let dest = rootfs_dir.join(&guest_path);
				fs::copy(module_path, &dest)
					.unwrap_or_else(|e| panic!("Failed to copy {module_path}: {e}"));
			}
		}

		// Auto-discover TDX kernel modules — copy into rootfs
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
			let n = stage_auto_discovered_modules(&rootfs_dir, modules_dir);
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

		// Build CPIO using system cpio — handles merged-usr
		// symlinks, permissions, and all edge cases correctly.
		eprintln!("==> Assembling CPIO from Ubuntu rootfs...");
		let cpio_path = out_dir.join("initramfs.cpio");

		let cpio_file = std::io::BufWriter::new(File::create(&cpio_path).unwrap());
		let mut cpio = super::super::cpio::CpioWriter::new(cpio_file);
		let entry_count = add_directory_to_cpio(&mut cpio, &rootfs_dir, "");
		let writer = cpio.finish().unwrap();
		writer.into_inner().unwrap().sync_all().unwrap();

		eprintln!("  [cpio] {entry_count} entries written");

		// Verify the CPIO contains critical entries
		let verify = Command::new("cpio")
			.args(["-itv"])
			.stdin(File::open(&cpio_path).unwrap())
			.output();
		if let Ok(out) = verify {
			let listing = String::from_utf8_lossy(&out.stdout);
			for name in ["init", "bin", "lib", "lib64", "sbin"] {
				let found = listing.lines().any(|l| {
					l.ends_with(&format!(" {name}"))
						|| l.contains(&format!(" {name} -> "))
				});
				eprintln!(
					"  [cpio] /{name}: {}",
					if found { "OK" } else { "MISSING!" }
				);
			}
		}

		// Clean up extracted rootfs
		let _ = fs::remove_dir_all(&rootfs_dir);

		// ---------------------------------------------------
		// 7. gzip → final output
		// ---------------------------------------------------
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

		// ---------------------------------------------------
		// 8. Report initramfs
		// ---------------------------------------------------
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

		// ---------------------------------------------------
		// 9. Copy kernel to output directory
		// ---------------------------------------------------
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

		// ---------------------------------------------------
		// 10. Generate launch-tdx.sh helper script
		// ---------------------------------------------------
		generate_launch_script(
			&crate_name,
			&artifacts_dir,
			self.default_cpus,
			&self.default_memory,
			self.ssh_forward,
		);

		// ---------------------------------------------------
		// 11. Obtain ovmf.fd and precompute MRTD
		// ---------------------------------------------------
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

		// Precompute RTMR[1] (kernel boot measurement)
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

		// Precompute RTMR[2] (initrd + cmdline measurement)
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

		// ---------------------------------------------------
		// 12. Generate self-extracting run-qemu.sh
		// ---------------------------------------------------
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

impl Default for UbuntuBuilder {
	fn default() -> Self {
		Self {
			ubuntu_version: None,
			custom_rootfs: None,
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

// -----------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------

/// Generate the Ubuntu-specific `/init` script.
fn generate_ubuntu_init_script(
	crate_name: &str,
	ssh_keys: &[String],
) -> String {
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

/// Download and extract Ubuntu .deb packages that are missing from
/// the minbase rootfs but required for TDX guest init (kmod for
/// insmod/modprobe, iproute2 for ip).
fn install_packages(cache_dir: &Path, rootfs_dir: &Path) {
	// Packages required for TDX guest init that are not in
	// ubuntu-base (debootstrap --variant=minbase).
	//
	// Each entry: (label, archive URL prefix, deb filename).
	// Versions pinned to Ubuntu 24.04 Noble.
	const ARCHIVE: &str = "https://archive.ubuntu.com/ubuntu/pool";

	let packages: &[(&str, &str)] = &[
		// kmod: insmod, modprobe, lsmod
		("kmod", "main/k/kmod/kmod_31+20240202-2ubuntu7_amd64.deb"),
		(
			"libkmod2",
			"main/k/kmod/libkmod2_31+20240202-2ubuntu7_amd64.deb",
		),
		// iproute2: ip
		(
			"iproute2",
			"main/i/iproute2/iproute2_6.1.0-1ubuntu6_amd64.deb",
		),
		// iproute2 runtime deps that may be absent from minbase
		(
			"libmnl0",
			"main/libm/libmnl/libmnl0_1.0.4-3build2_amd64.deb",
		),
		(
			"libelf1t64",
			"main/e/elfutils/libelf1t64_0.190-1.1build4_amd64.deb",
		),
		(
			"libbpf1",
			"main/libb/libbpf/libbpf1_1.3.0-2build2_amd64.deb",
		),
		(
			"libcap2-bin",
			"main/libc/libcap2/libcap2-bin_2.66-5ubuntu2_amd64.deb",
		),
	];

	eprintln!("==> Installing extra packages into rootfs...");

	let pkg_cache = cache_dir.join("packages");
	fs::create_dir_all(&pkg_cache).unwrap();

	// Extract each .deb into a staging directory, then merge
	// into the rootfs. The Ubuntu rootfs uses merged-usr symlinks
	// (bin → usr/bin, lib → usr/lib, sbin → usr/sbin), but .deb
	// packages ship files under /bin, /lib, /sbin as real paths.
	// Extracting directly into the rootfs would replace the
	// symlinks with real directories, breaking the layout.
	// Instead we extract to a staging area and copy files into
	// the rootfs, following its symlinks so files land in the
	// correct usr/* location.
	for (label, deb_path) in packages {
		let url = format!("{ARCHIVE}/{deb_path}");
		let filename = deb_path.rsplit('/').next().unwrap();
		let deb = download_cached(&url, &pkg_cache, filename);

		let tmp = pkg_cache.join(format!("tmp-{label}"));
		let _ = fs::remove_dir_all(&tmp);
		fs::create_dir_all(&tmp).unwrap();

		let ar_ok = Command::new("ar")
			.args(["x"])
			.arg(&deb)
			.current_dir(&tmp)
			.status()
			.map(|s| s.success())
			.unwrap_or(false);

		if ar_ok {
			if let Some(data_tar) = find_file_starting_with(&tmp, "data.tar") {
				// Extract to a staging dir, then merge
				let staging = pkg_cache.join(format!("stage-{label}"));
				let _ = fs::remove_dir_all(&staging);
				fs::create_dir_all(&staging).unwrap();
				extract_data_tar(&data_tar, &staging);
				merge_into_rootfs(&staging, rootfs_dir);
				let _ = fs::remove_dir_all(&staging);
				eprintln!("  [pkg] {label}");
			} else {
				eprintln!("  [pkg] {label}: no data.tar.* in .deb");
			}
		} else {
			eprintln!("  [pkg] {label}: ar extraction failed");
		}

		let _ = fs::remove_dir_all(&tmp);
	}
}

/// Recursively copy files from `src` into `dst`, following
/// symlinks in `dst` so that merged-usr symlinks are preserved.
///
/// For example, if `dst/bin` is a symlink to `usr/bin` and `src`
/// contains `bin/ip`, this copies `ip` into `dst/usr/bin/ip`
/// (through the symlink) rather than replacing the `bin` symlink
/// with a directory.
fn merge_into_rootfs(src: &Path, dst: &Path) {
	let Ok(entries) = fs::read_dir(src) else {
		return;
	};
	for entry in entries.filter_map(|e| e.ok()) {
		let name = entry.file_name();
		let src_path = entry.path();
		// Use the real (symlink-following) path in dst so that
		// e.g. dst/bin (symlink → usr/bin) resolves to
		// dst/usr/bin and we write into the real directory.
		let dst_path = dst.join(&name);

		let Ok(src_meta) = fs::symlink_metadata(&src_path) else {
			continue;
		};

		if src_meta.is_dir() {
			// If the destination is a symlink to a directory,
			// fs::create_dir_all follows it — which is exactly
			// what we want for merged-usr.
			fs::create_dir_all(&dst_path).unwrap();
			merge_into_rootfs(&src_path, &dst_path);
		} else if src_meta.file_type().is_symlink() {
			let target = fs::read_link(&src_path).unwrap();
			// Remove any existing entry and create symlink
			let _ = fs::remove_file(&dst_path);
			#[cfg(unix)]
			std::os::unix::fs::symlink(&target, &dst_path)
				.unwrap();
		} else {
			// Regular file — copy into dst (follows dst symlinks)
			fs::copy(&src_path, &dst_path).unwrap();
		}
	}
}

/// Set a file as executable (Unix only).
fn set_executable(path: &Path) {
	let mut perms = fs::metadata(path).unwrap().permissions();
	perms.set_mode(perms.mode() | 0o111);
	fs::set_permissions(path, perms).unwrap();
}

/// Find and copy auto-discovered TDX kernel modules into the
/// rootfs `lib/modules/` directory, decompressing `.zst`/`.xz` as
/// needed. Returns the number of modules copied.
fn stage_auto_discovered_modules(rootfs_dir: &Path, modules_dir: &Path) -> u32 {
	eprintln!(
		"==> Auto-discovering TDX kernel modules from {}...",
		modules_dir.display()
	);

	let wanted = [
		"configfs",
		"tsm",
		"tdx_guest",
		"tdx-guest",
		"vsock",
		"vmw_vsock_virtio_transport",
		"vmw_vsock_virtio_transport_common",
	];

	let dest_dir = rootfs_dir.join("lib/modules");
	fs::create_dir_all(&dest_dir).unwrap();

	let mut count: u32 = 0;

	for module_name in &wanted {
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

		let Some(ko_path) = found_path else {
			eprintln!("  [module] {module_name}.ko not found (may be built-in)");
			continue;
		};

		let ko_filename =
			Path::new(&ko_path).file_name().unwrap().to_str().unwrap();

		let (data, guest_name) = if Path::new(ko_filename)
			.extension()
			.is_some_and(|ext| ext.eq_ignore_ascii_case("zst"))
		{
			let out = Command::new("zstd")
				.args(["-d", "-c"])
				.arg(&ko_path)
				.output()
				.expect("failed to run zstd");
			assert!(out.status.success(), "zstd failed on {ko_path}");
			(
				out.stdout,
				ko_filename.strip_suffix(".zst").unwrap().to_string(),
			)
		} else if Path::new(ko_filename)
			.extension()
			.is_some_and(|ext| ext.eq_ignore_ascii_case("xz"))
		{
			let out = Command::new("xz")
				.args(["-d", "-c"])
				.arg(&ko_path)
				.output()
				.expect("failed to run xz");
			assert!(out.status.success(), "xz failed on {ko_path}");
			(
				out.stdout,
				ko_filename.strip_suffix(".xz").unwrap().to_string(),
			)
		} else {
			let data = fs::read(&ko_path)
				.unwrap_or_else(|e| panic!("Failed to read {ko_path}: {e}"));
			(data, ko_filename.to_string())
		};

		eprintln!(
			"  [module] {ko_path} -> /lib/modules/{guest_name} ({:.0} KB)",
			data.len() as f64 / 1024.0
		);
		fs::write(dest_dir.join(&guest_name), &data).unwrap();
		count += 1;
	}

	count
}

/// Recursively walk a directory and add all entries to the CPIO
/// writer. Entries are sorted for reproducibility. Uses
/// `symlink_metadata` so symlinks (e.g. merged-usr `/bin` →
/// `usr/bin`) are stored as symlinks rather than followed.
fn add_directory_to_cpio<W: std::io::Write>(
	cpio: &mut super::super::cpio::CpioWriter<W>,
	root: &Path,
	prefix: &str,
) -> u32 {
	let mut count: u32 = 0;
	let mut entries: Vec<_> = fs::read_dir(root)
		.unwrap_or_else(|e| {
			panic!("Failed to read directory {}: {e}", root.display())
		})
		.filter_map(|e| e.ok())
		.collect();
	entries.sort_by_key(|e| e.file_name());

	for entry in entries {
		let name = entry.file_name().to_string_lossy().to_string();
		let path = entry.path();
		let cpio_path = if prefix.is_empty() {
			name.clone()
		} else {
			format!("{prefix}/{name}")
		};

		let Ok(meta) = fs::symlink_metadata(&path) else {
			continue;
		};

		if meta.file_type().is_symlink() {
			let target = fs::read_link(&path).unwrap();
			let target_str = target.to_string_lossy();
			cpio.add_symlink(&cpio_path, &target_str).unwrap();
			count += 1;
		} else if meta.is_dir() {
			cpio.add_dir(&cpio_path).unwrap();
			count += 1;
			count += add_directory_to_cpio(cpio, &path, &cpio_path);
		} else if meta.is_file() {
			let data = fs::read(&path)
				.unwrap_or_else(|e| panic!("Failed to read {}: {e}", path.display()));
			let executable = meta.permissions().mode() & 0o111 != 0;
			cpio.add_file(&cpio_path, &data, executable).unwrap();
			count += 1;
		}
	}
	count
}
