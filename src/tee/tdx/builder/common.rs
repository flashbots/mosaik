//! Shared infrastructure for TDX image builders.

use {
	super::{
		BuilderOutput,
		helpers::{detect_profile, env_or, find_target_dir},
		kernel::auto_download_kernel,
		mrtd,
		ovmf::obtain_ovmf,
		scripts::{generate_launch_script, generate_self_extracting_script},
	},
	crate::tee::tdx::Measurement,
	std::{
		env,
		fs::{self, File},
		path::{Path, PathBuf},
		process::Command,
	},
};

/// Fields shared by all TDX image builders.
pub(super) struct CommonConfig {
	pub custom_vmlinuz: Option<Vec<u8>>,
	pub custom_ovmf: Option<Vec<u8>>,
	pub ssh_keys: Vec<String>,
	pub ssh_forward: Option<(u16, u16)>,
	pub default_cpus: u32,
	pub default_memory: String,
	pub bundle_runner: bool,
	pub extra_files: Vec<(PathBuf, String)>,
	pub extra_kernel_modules: Vec<PathBuf>,
	pub kernel_modules_dir: Option<PathBuf>,
	pub kernel_version: Option<String>,
	pub kernel_abi: Option<String>,
	pub ovmf_version: Option<String>,
	pub artifacts_output_path: Option<PathBuf>,
}

impl Default for CommonConfig {
	fn default() -> Self {
		Self {
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

/// Resolved build environment from Cargo.
pub(super) struct BuildContext {
	pub out_dir: PathBuf,
	pub cache_dir: PathBuf,
	pub crate_name: String,
	pub profile: &'static str,
	pub target_dir: PathBuf,
}

impl BuildContext {
	pub fn resolve() -> Self {
		let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
		let cache_dir = out_dir.join("tdx-image-cache");
		fs::create_dir_all(&cache_dir).unwrap();
		Self {
			out_dir,
			cache_dir,
			crate_name: env::var("CARGO_PKG_NAME").unwrap(),
			profile: detect_profile(),
			target_dir: find_target_dir(),
		}
	}
}

impl CommonConfig {
	/// Resolve the artifacts output directory.
	pub fn resolve_artifacts_dir(
		&self,
		ctx: &BuildContext,
		distro: &str,
	) -> PathBuf {
		let dir = match self.artifacts_output_path {
			Some(ref p) if p.is_absolute() => p.clone(),
			Some(ref p) => ctx.target_dir.join(p),
			None => ctx
				.target_dir
				.join(ctx.profile)
				.join("tdx-artifacts")
				.join(&ctx.crate_name)
				.join(distro),
		};
		fs::create_dir_all(&dir).unwrap();
		dir
	}
}

/// Returns `true` if the build should be skipped (recursion guard
/// or `TDX_IMAGE_SKIP=1`).
pub(super) fn check_skip_build() -> bool {
	if env::var("__TDX_IMAGE_INNER_BUILD").is_ok() {
		return true;
	}
	println!("cargo:rerun-if-env-changed=TDX_IMAGE_SKIP");
	println!("cargo:rerun-if-env-changed=TDX_IMAGE_EXTRA_FILES");
	println!("cargo:rerun-if-env-changed=TDX_IMAGE_KERNEL_MODULES");
	if env_or("TDX_IMAGE_SKIP", "0") == "1" {
		eprintln!("TDX_IMAGE_SKIP=1 — skipping initramfs build");
		return true;
	}
	false
}

/// Acquire the kernel image and auto-download module directory.
///
/// Returns `(kernel_vmlinuz, auto_modules_dir)`.
pub(super) fn acquire_kernel(
	config: &CommonConfig,
	kernel_cache_dir: &Path,
) -> (Option<PathBuf>, Option<PathBuf>) {
	println!("cargo:rerun-if-env-changed=TDX_IMAGE_KERNEL");
	println!("cargo:rerun-if-env-changed=TDX_IMAGE_KERNEL_VERSION");

	let kernel_vmlinuz: Option<PathBuf>;
	let mut auto_modules_dir: Option<PathBuf> = None;

	if let Some(ref custom_vmlinuz) = config.custom_vmlinuz {
		let path = kernel_cache_dir.join("custom-vmlinuz");
		fs::write(&path, custom_vmlinuz).unwrap();
		kernel_vmlinuz = Some(path);
	} else if let Ok(kernel_path) = env::var("TDX_IMAGE_KERNEL") {
		eprintln!("==> Using user-provided kernel: {kernel_path}");
		kernel_vmlinuz = Some(PathBuf::from(&kernel_path));
	} else {
		let (vmlinuz, modules_dir) = auto_download_kernel(
			kernel_cache_dir,
			config.kernel_version.as_deref(),
			config.kernel_abi.as_deref(),
		);
		kernel_vmlinuz = vmlinuz;
		auto_modules_dir = modules_dir;
	}

	let has_explicit_modules_dir = config.kernel_modules_dir.is_some()
		|| env::var("TDX_IMAGE_KERNEL_MODULES_DIR").is_ok()
		|| !config.extra_kernel_modules.is_empty();

	if auto_modules_dir.is_none() && !has_explicit_modules_dir {
		eprintln!(
			"==> No modules directory — auto-downloading Ubuntu kernel modules for \
			 TDX support..."
		);
		let (_, modules_dir) = auto_download_kernel(
			kernel_cache_dir,
			config.kernel_version.as_deref(),
			config.kernel_abi.as_deref(),
		);
		auto_modules_dir = modules_dir;
	}

	(kernel_vmlinuz, auto_modules_dir)
}

/// Resolve the effective kernel modules directory from builder
/// config, environment, or auto-downloaded modules.
pub(super) fn resolve_modules_dir(
	config: &CommonConfig,
	auto_modules_dir: Option<&Path>,
) -> Option<PathBuf> {
	println!("cargo:rerun-if-env-changed=TDX_IMAGE_KERNEL_MODULES_DIR");

	config
		.kernel_modules_dir
		.clone()
		.or_else(|| {
			env::var("TDX_IMAGE_KERNEL_MODULES_DIR")
				.map(PathBuf::from)
				.ok()
		})
		.or_else(|| auto_modules_dir.map(Path::to_path_buf))
}

/// Post-CPIO build pipeline: gzip, kernel copy, OVMF, measurements,
/// launch scripts, and `BuilderOutput` construction.
#[allow(clippy::too_many_lines)]
pub(super) fn finalize_build(
	config: &CommonConfig,
	ctx: &BuildContext,
	cpio_path: &Path,
	artifacts_dir: &Path,
	kernel_vmlinuz: Option<&Path>,
) -> BuilderOutput {
	let output_filename = format!("{}-initramfs.cpio.gz", ctx.crate_name);
	let final_path = artifacts_dir.join(&output_filename);

	// gzip → final output
	eprintln!("==> Compressing...");
	let gz_path = ctx.out_dir.join(&output_filename);

	let gz_file = File::create(&gz_path).unwrap();
	let status = Command::new("gzip")
		.args(["-9", "-n", "-c"])
		.arg(cpio_path)
		.stdout(gz_file)
		.status()
		.expect("failed to run gzip — is it installed?");

	assert!(status.success(), "gzip compression failed");

	fs::copy(&gz_path, &final_path).unwrap();

	// Report initramfs
	let cpio_size = fs::metadata(cpio_path).map(|m| m.len()).unwrap_or(0);
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

	let _ = fs::remove_file(cpio_path);

	// Copy kernel to output directory
	let kernel_output = artifacts_dir.join(format!("{}-vmlinuz", ctx.crate_name));

	if let Some(vmlinuz) = kernel_vmlinuz {
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

	// Generate launch-tdx.sh
	generate_launch_script(
		&ctx.crate_name,
		artifacts_dir,
		config.default_cpus,
		&config.default_memory,
		config.ssh_forward,
	);

	// Obtain OVMF and precompute measurements
	let kernel_cache_dir = ctx.cache_dir.join("kernel");
	let ovmf_output = artifacts_dir.join(format!("{}-ovmf.fd", ctx.crate_name));

	let ovmf_data = obtain_ovmf(
		&config.custom_ovmf,
		&kernel_cache_dir,
		config.ovmf_version.as_deref(),
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

				let mrtd_path =
					artifacts_dir.join(format!("{}-mrtd.hex", ctx.crate_name));
				fs::write(&mrtd_path, &hex).unwrap();
				println!("cargo:warning=MRTD written to: {}", mrtd_path.display());
			}
			Err(e) => {
				println!("cargo:warning=MRTD computation failed: {e}");
			}
		}
	}

	// RTMR[1]
	if kernel_output.exists() && final_path.exists() {
		eprintln!("==> Precomputing RTMR[1]...");
		let kernel_data = fs::read(&kernel_output).unwrap();
		let initrd_data = fs::read(&final_path).unwrap();
		rtmr1_value =
			mrtd::compute_rtmr1(&kernel_data, &initrd_data, &config.default_memory);
		let hex = hex::encode(rtmr1_value);
		println!("cargo:warning=RTMR[1]: {hex}");
		println!("cargo:rustc-env=TDX_EXPECTED_RTMR1={hex}");

		let rtmr1_path =
			artifacts_dir.join(format!("{}-rtmr1.hex", ctx.crate_name));
		fs::write(&rtmr1_path, &hex).unwrap();
		println!("cargo:warning=RTMR[1] written to: {}", rtmr1_path.display(),);
	}

	// RTMR[2]
	if final_path.exists() {
		eprintln!("==> Precomputing RTMR[2]...");
		let initrd_data = fs::read(&final_path).unwrap();
		let cmdline = "console=ttyS0 ip=dhcp";
		rtmr2_value = mrtd::compute_rtmr2(cmdline, &initrd_data);
		let hex = hex::encode(rtmr2_value);
		println!("cargo:warning=RTMR[2]: {hex}");
		println!("cargo:rustc-env=TDX_EXPECTED_RTMR2={hex}");

		let rtmr2_path =
			artifacts_dir.join(format!("{}-rtmr2.hex", ctx.crate_name));
		fs::write(&rtmr2_path, &hex).unwrap();
		println!("cargo:warning=RTMR[2] written to: {}", rtmr2_path.display());
	}

	// Self-extracting bundle
	generate_self_extracting_script(
		&ctx.crate_name,
		artifacts_dir,
		&ctx.out_dir,
		&kernel_output,
		&final_path,
		&ovmf_output,
		config.default_cpus,
		&config.default_memory,
		config.ssh_forward,
	);

	let bundle_path =
		artifacts_dir.join(format!("{}-run-qemu.sh", ctx.crate_name));

	BuilderOutput {
		initramfs_path: final_path,
		kernel_path: kernel_output,
		ovmf_path: ovmf_output,
		bundle_path,
		mrtd: Measurement::from(mrtd_value),
		rtmr1: Measurement::from(rtmr1_value),
		rtmr2: Measurement::from(rtmr2_value),
	}
}
