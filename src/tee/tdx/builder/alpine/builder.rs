//! `AlpineBuilder` — fluent API for assembling TDX guest images.

use {
	super::{
		super::{
			common::{
				BuildContext,
				CommonConfig,
				acquire_kernel,
				check_skip_build,
				finalize_build,
				resolve_modules_dir,
			},
			cpio::CpioWriter,
			helpers::{
				download_cached,
				env_or,
				extract_file_from_tar_gz,
				list_tar_gz,
			},
			kernel::bundle_auto_discovered_modules,
		},
		cross::cross_compile_musl,
	},
	std::{
		env,
		fs::{self, File},
		io::BufWriter,
		path::PathBuf,
	},
};

/// Builder for Alpine-based TDX guest images.
///
/// Constructed via [`mosaik::tee::tdx::build::alpine()`].
#[derive(Default)]
pub struct AlpineBuilder {
	pub(super) common: CommonConfig,
	pub(super) alpine_major: Option<String>,
	pub(super) alpine_minor: Option<String>,
	pub(super) custom_minirootfs: Option<Vec<u8>>,
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
		self.common.custom_vmlinuz = Some(bytes.to_vec());
		self
	}

	/// Provide a custom ovmf.fd firmware (in-memory bytes).
	/// When set, skips auto-downloading OVMF.
	#[must_use]
	pub fn with_custom_ovmf(mut self, bytes: &[u8]) -> Self {
		self.common.custom_ovmf = Some(bytes.to_vec());
		self
	}

	/// Enable SSH access in the guest by forwarding a host port to
	/// the guest SSH daemon.
	#[must_use]
	pub const fn with_ssh_forward(
		mut self,
		host_port: u16,
		guest_port: u16,
	) -> Self {
		self.common.ssh_forward = Some((host_port, guest_port));
		self
	}

	/// Add an SSH public key to the guest's
	/// `/root/.ssh/authorized_keys`.
	#[must_use]
	pub fn with_ssh_key(mut self, pubkey: &str) -> Self {
		self.common.ssh_keys.push(pubkey.to_string());
		self
	}

	/// Set the default vCPU count for the QEMU launch script.
	/// Defaults to 4.
	#[must_use]
	pub const fn with_default_cpu_count(mut self, count: u32) -> Self {
		self.common.default_cpus = count;
		self
	}

	/// Set the default memory size for the QEMU launch script
	/// (e.g. `"4G"`, `"8G"`). Defaults to `"4G"`.
	#[must_use]
	pub fn with_default_memory_size(mut self, size: &str) -> Self {
		self.common.default_memory = size.to_string();
		self
	}

	/// Include the mosaik bundle runner in the guest image.
	/// This is the default.
	#[must_use]
	pub const fn with_bundle_runner(mut self) -> Self {
		self.common.bundle_runner = true;
		self
	}

	/// Exclude the mosaik bundle runner from the guest image.
	#[must_use]
	pub const fn without_bundle_runner(mut self) -> Self {
		self.common.bundle_runner = false;
		self
	}

	/// Add an extra file to bundle into the initramfs.
	#[must_use]
	pub fn with_extra_file(
		mut self,
		host_path: impl Into<PathBuf>,
		guest_path: &str,
	) -> Self {
		self.common.extra_files.push((
			host_path.into(),
			guest_path.trim_start_matches('/').to_string(),
		));
		self
	}

	/// Add a kernel module (.ko file) to bundle into the initramfs.
	#[must_use]
	pub fn with_kernel_module(mut self, path: impl Into<PathBuf>) -> Self {
		self.common.extra_kernel_modules.push(path.into());
		self
	}

	/// Set the directory to auto-discover TDX kernel modules from.
	#[must_use]
	pub fn with_kernel_modules_dir(mut self, path: impl Into<PathBuf>) -> Self {
		self.common.kernel_modules_dir = Some(path.into());
		self
	}

	/// Set the Ubuntu kernel version for auto-download
	/// (e.g. `"6.8.0-55"`). Defaults to `"6.8.0-55"`.
	#[must_use]
	pub fn with_kernel_version(mut self, version: &str) -> Self {
		self.common.kernel_version = Some(version.to_string());
		self
	}

	/// Set the Ubuntu kernel ABI number for auto-download
	/// (e.g. `"57"`). Defaults to `"57"`.
	#[must_use]
	pub fn with_kernel_abi(mut self, abi: &str) -> Self {
		self.common.kernel_abi = Some(abi.to_string());
		self
	}

	/// Set the OVMF version for auto-download
	/// (e.g. `"2024.02-3+tdx1.0"`).
	#[must_use]
	pub fn with_ovmf_version(mut self, version: &str) -> Self {
		self.common.ovmf_version = Some(version.to_string());
		self
	}

	/// Set a custom path for build artifacts.
	///
	/// Defaults to `<target_dir>/tdx-artifacts/alpine`.
	#[must_use]
	pub fn with_artifacts_output_path(
		mut self,
		path: impl Into<PathBuf>,
	) -> Self {
		self.common.artifacts_output_path = Some(path.into());
		self
	}

	/// Append a command-line argument passed to the binary at
	/// startup inside the guest.
	///
	/// Arguments are baked into the `/init` script in the order
	/// they are added. When no arguments are configured the
	/// kernel command-line pass-through (`"$@"`) is preserved.
	#[must_use]
	pub fn with_arg(mut self, arg: &str) -> Self {
		self.common.args.push(arg.to_string());
		self
	}

	/// Append multiple command-line arguments at once.
	#[must_use]
	pub fn with_args(mut self, args: &[&str]) -> Self {
		self.common.args.extend(args.iter().map(|a| (*a).to_string()));
		self
	}

	/// Set an environment variable that will be exported before
	/// the binary is started inside the guest.
	#[must_use]
	pub fn with_env(mut self, key: &str, value: &str) -> Self {
		self.common
			.env_vars
			.push((key.to_string(), value.to_string()));
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
		if check_skip_build() {
			return None;
		}

		println!("cargo:rerun-if-env-changed=TDX_IMAGE_ALPINE_VERSION");
		println!("cargo:rerun-if-env-changed=TDX_IMAGE_ALPINE_MINOR");

		let ctx = BuildContext::resolve();
		let artifacts_dir = self.common.resolve_artifacts_dir(&ctx, "alpine");

		let alpine_ver = self
			.alpine_major
			.unwrap_or_else(|| env_or("TDX_IMAGE_ALPINE_VERSION", "3.21"));
		let alpine_minor = self
			.alpine_minor
			.unwrap_or_else(|| env_or("TDX_IMAGE_ALPINE_MINOR", "0"));

		eprintln!(
			"==> TDX initramfs: profile={}, crate={}",
			ctx.profile, ctx.crate_name
		);

		// --- Cross-compile for musl ---
		let binary_data =
			cross_compile_musl(&ctx.crate_name, ctx.profile, &ctx.target_dir);

		// --- Download Alpine minirootfs ---
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
				download_cached(&alpine_url, &ctx.cache_dir, &alpine_tar_name)
			},
			|custom_minirootfs| {
				let path = ctx.cache_dir.join("custom-minirootfs.tar.gz");
				fs::write(&path, custom_minirootfs).unwrap();
				path
			},
		);

		// --- Extract busybox + musl libs ---
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

		// --- Generate /init ---
		eprintln!("==> Generating /init...");
		let init_script =
			generate_init_script(&ctx.crate_name, &self.common);

		// --- Kernel + modules ---
		let kernel_cache_dir = ctx.cache_dir.join("kernel");
		fs::create_dir_all(&kernel_cache_dir).unwrap();

		let (kernel_vmlinuz, auto_modules_dir) =
			acquire_kernel(&self.common, &kernel_cache_dir);

		// --- Assemble CPIO ---
		eprintln!("==> Assembling CPIO...");

		let cpio_path = ctx.out_dir.join("initramfs.cpio");
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
			.add_file(&format!("usr/bin/{}", ctx.crate_name), &binary_data, true)
			.unwrap();

		// Extra files from builder API
		for (host_path, guest_path) in &self.common.extra_files {
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
		for module_path in &self.common.extra_kernel_modules {
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
				let filename = std::path::Path::new(module_path)
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
		let effective_modules_dir =
			resolve_modules_dir(&self.common, auto_modules_dir.as_deref());

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

		// --- Post-CPIO pipeline (shared) ---
		Some(finalize_build(
			&self.common,
			&ctx,
			&cpio_path,
			&artifacts_dir,
			kernel_vmlinuz.as_deref(),
		))
	}
}

// Default derived: all fields are None / CommonConfig::default().

const UDHCPC_SCRIPT: &str = include_str!("templates/udhcpc.sh");

fn generate_init_script(
	crate_name: &str,
	common: &CommonConfig,
) -> String {
	let ssh_block = if common.ssh_keys.is_empty() {
		String::new()
	} else {
		include_str!("../templates/init-ssh.sh")
			.replace("{{SSH_KEYS}}", &common.ssh_keys.join("\n"))
	};

	include_str!("templates/init.sh")
		.replace("{{CRATE_NAME}}", crate_name)
		.replace("{{SSH_BLOCK}}", &ssh_block)
		.replace("{{ENV_BLOCK}}", &common.env_block())
		.replace("{{ARGS}}", &common.args_string())
}
