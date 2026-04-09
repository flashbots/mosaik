//! Kernel download and module discovery for TDX guests.

use {
	super::{
		cpio::CpioWriter,
		helpers::{
			download_cached,
			env_or,
			extract_data_tar,
			find_file_recursive,
			find_file_starting_with,
		},
	},
	std::{
		fs,
		io::Write,
		path::{Path, PathBuf},
		process::Command,
	},
};

pub(super) fn auto_download_kernel(
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

pub(super) fn bundle_auto_discovered_modules<W: Write>(
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
