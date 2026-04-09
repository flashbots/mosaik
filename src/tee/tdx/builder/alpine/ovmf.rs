//! OVMF firmware acquisition for TDX guests.

use {
	super::helpers::{
		env_or,
		extract_data_tar,
		find_file_recursive,
		find_file_starting_with,
	},
	std::{env, fs, path::Path, process::Command},
};

#[allow(clippy::ref_option)]
pub(super) fn obtain_ovmf(
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
			panic!("Failed to read ovmf.fd at {ovmf_path}: {e}")
		}));
	}

	eprintln!("==> Auto-downloading ovmf.fd...");

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
