//! `UbuntuBuilder` — fluent API for assembling glibc-based TDX guest
//! images.

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
				extract_data_tar,
				find_file_starting_with,
			},
		},
		cross::cross_compile_gnu,
	},
	std::{
		env,
		fs::{self, File},
		io::BufWriter,
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
#[derive(Default)]
pub struct UbuntuBuilder {
	pub(super) common: CommonConfig,
	pub(super) ubuntu_version: Option<String>,
	pub(super) custom_rootfs: Option<Vec<u8>>,
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
	/// Defaults to `<target_dir>/tdx-artifacts/ubuntu`.
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

		println!("cargo:rerun-if-env-changed=TDX_IMAGE_UBUNTU_VERSION");

		let ctx = BuildContext::resolve();
		let artifacts_dir = self.common.resolve_artifacts_dir(&ctx, "ubuntu");

		eprintln!(
			"==> TDX initramfs (ubuntu): profile={}, crate={}",
			ctx.profile, ctx.crate_name
		);

		// --- Cross-compile for gnu ---
		let binary_data =
			cross_compile_gnu(&ctx.crate_name, ctx.profile, &ctx.target_dir);

		// --- Download Ubuntu minimal rootfs ---
		let ubuntu_ver = self
			.ubuntu_version
			.unwrap_or_else(|| env_or("TDX_IMAGE_UBUNTU_VERSION", "24.04.4"));

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
				download_cached(&ubuntu_url, &ctx.cache_dir, &ubuntu_tar_name)
			},
			|custom_rootfs| {
				let path = ctx.cache_dir.join("custom-rootfs.tar.gz");
				fs::write(&path, custom_rootfs).unwrap();
				path
			},
		);

		// --- Extract rootfs ---
		eprintln!("==> Extracting Ubuntu rootfs...");
		let rootfs_dir = ctx.out_dir.join("ubuntu-rootfs");
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

		// --- Install packages missing from minbase ---
		install_packages(&ctx.cache_dir, &rootfs_dir);

		// --- Generate /init and overlay custom files ---
		eprintln!("==> Generating /init...");
		let init_script =
			generate_ubuntu_init_script(&ctx.crate_name, &self.common);
		fs::write(rootfs_dir.join("init"), &init_script).unwrap();
		set_executable(&rootfs_dir.join("init"));

		for dir in &["lib/modules", "sys/kernel/config", "dev/pts", "dev/shm"] {
			fs::create_dir_all(rootfs_dir.join(dir)).unwrap();
		}

		fs::create_dir_all(rootfs_dir.join("usr/bin")).unwrap();
		fs::write(
			rootfs_dir.join(format!("usr/bin/{}", ctx.crate_name)),
			&binary_data,
		)
		.unwrap();
		set_executable(&rootfs_dir.join(format!("usr/bin/{}", ctx.crate_name)));

		// --- Kernel + modules ---
		let kernel_cache_dir = ctx.cache_dir.join("kernel");
		fs::create_dir_all(&kernel_cache_dir).unwrap();

		let (kernel_vmlinuz, auto_modules_dir) =
			acquire_kernel(&self.common, &kernel_cache_dir);

		// --- Stage extra files into rootfs ---
		for (host_path, guest_path) in &self.common.extra_files {
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

		for module_path in &self.common.extra_kernel_modules {
			let filename = module_path.file_name().unwrap().to_str().unwrap();
			let guest_path = format!("lib/modules/{filename}");
			eprintln!("  [module] {} → /{guest_path}", module_path.display());
			println!("cargo:rerun-if-changed={}", module_path.display());
			fs::copy(module_path, rootfs_dir.join(&guest_path)).unwrap_or_else(|e| {
				panic!("Failed to copy {}: {e}", module_path.display())
			});
		}

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
				fs::copy(module_path, rootfs_dir.join(&guest_path))
					.unwrap_or_else(|e| panic!("Failed to copy {module_path}: {e}"));
			}
		}

		// Auto-discover TDX kernel modules
		let effective_modules_dir =
			resolve_modules_dir(&self.common, auto_modules_dir.as_deref());

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

		if let Some(ref dir) = auto_modules_dir {
			let _ = fs::remove_dir_all(dir);
		}

		// --- Assemble CPIO from rootfs ---
		eprintln!("==> Assembling CPIO from Ubuntu rootfs...");
		let cpio_path = ctx.out_dir.join("initramfs.cpio");

		let cpio_file = BufWriter::new(File::create(&cpio_path).unwrap());
		let mut cpio = CpioWriter::new(cpio_file);
		add_directory_to_cpio(&mut cpio, &rootfs_dir, "");
		let writer = cpio.finish().unwrap();
		writer.into_inner().unwrap().sync_all().unwrap();

		let _ = fs::remove_dir_all(&rootfs_dir);

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

// -----------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------

/// Generate the Ubuntu-specific `/init` script.
fn generate_ubuntu_init_script(
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

/// Download and extract Ubuntu .deb packages that are missing from
/// the minbase rootfs but required for TDX guest init (kmod for
/// insmod/modprobe, iproute2 for ip).
fn install_packages(cache_dir: &Path, rootfs_dir: &Path) {
	const ARCHIVE: &str = "https://archive.ubuntu.com/ubuntu/pool";

	let packages: &[(&str, &str)] = &[
		("kmod", "main/k/kmod/kmod_31+20240202-2ubuntu7_amd64.deb"),
		(
			"libkmod2",
			"main/k/kmod/libkmod2_31+20240202-2ubuntu7_amd64.deb",
		),
		(
			"iproute2",
			"main/i/iproute2/iproute2_6.1.0-1ubuntu6_amd64.deb",
		),
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

/// Recursively copy files from `src` into `dst`, following symlinks
/// in `dst` so that merged-usr symlinks are preserved.
fn merge_into_rootfs(src: &Path, dst: &Path) {
	let Ok(entries) = fs::read_dir(src) else {
		return;
	};
	for entry in entries.filter_map(|e| e.ok()) {
		let name = entry.file_name();
		let src_path = entry.path();
		let dst_path = dst.join(&name);

		let Ok(src_meta) = fs::symlink_metadata(&src_path) else {
			continue;
		};

		if src_meta.is_dir() {
			fs::create_dir_all(&dst_path).unwrap();
			merge_into_rootfs(&src_path, &dst_path);
		} else if src_meta.file_type().is_symlink() {
			let target = fs::read_link(&src_path).unwrap();
			let _ = fs::remove_file(&dst_path);
			#[cfg(unix)]
			std::os::unix::fs::symlink(&target, &dst_path).unwrap();
		} else {
			fs::copy(&src_path, &dst_path).unwrap();
		}
	}
}

fn set_executable(path: &Path) {
	let mut perms = fs::metadata(path).unwrap().permissions();
	perms.set_mode(perms.mode() | 0o111);
	fs::set_permissions(path, perms).unwrap();
}

/// Find and copy auto-discovered TDX kernel modules into the rootfs
/// `lib/modules/` directory, decompressing `.zst`/`.xz` as needed.
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
/// writer, using `symlink_metadata` so symlinks (merged-usr) are
/// stored as symlinks rather than followed.
fn add_directory_to_cpio<W: std::io::Write>(
	cpio: &mut CpioWriter<W>,
	root: &Path,
	prefix: &str,
) {
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
		} else if meta.is_dir() {
			cpio.add_dir(&cpio_path).unwrap();
			add_directory_to_cpio(cpio, &path, &cpio_path);
		} else if meta.is_file() {
			let data = fs::read(&path)
				.unwrap_or_else(|e| panic!("Failed to read {}: {e}", path.display()));
			let executable = meta.permissions().mode() & 0o111 != 0;
			cpio.add_file(&cpio_path, &data, executable).unwrap();
		}
	}
}
