//! Shared utility functions for the Alpine TDX builder.

use std::{
	env,
	fs,
	path::{Path, PathBuf},
	process::{Command, Stdio},
};

pub(super) fn env_or(key: &str, default: &str) -> String {
	env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Build the QEMU `-netdev user` host-forward fragment from the
/// optional `(host_port, guest_port)` pair.
///
/// When no forwarding is configured the generated scripts fall back to
/// the `$SSH_PORT` env-var (default 10022) → guest :22.
pub(super) fn ssh_fwd_rule(ssh_forward: Option<(u16, u16)>) -> String {
	match ssh_forward {
		Some((host, guest)) => {
			format!("hostfwd=tcp::{host}-:{guest}")
		}
		None => "hostfwd=tcp::\"$SSH_PORT\"-:22".to_string(),
	}
}

pub(super) fn download_cached(
	url: &str,
	cache_dir: &Path,
	filename: &str,
) -> PathBuf {
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

pub(super) fn extract_file_from_tar_gz(
	archive: &Path,
	inner_path: &str,
) -> Vec<u8> {
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

pub(super) fn list_tar_gz(archive: &Path) -> Vec<String> {
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
pub(super) fn extract_data_tar(data_tar: &Path, target_dir: &Path) {
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

pub(super) fn find_file_starting_with(
	dir: &Path,
	prefix: &str,
) -> Option<PathBuf> {
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

pub(super) fn find_file_recursive(
	dir: &Path,
	name_fragment: &str,
) -> Option<PathBuf> {
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

pub(super) fn detect_profile() -> &'static str {
	let out_dir = env::var("OUT_DIR").unwrap();
	if out_dir.contains("/release/") || out_dir.contains("\\release\\") {
		"release"
	} else {
		"debug"
	}
}

pub(super) fn find_target_dir() -> PathBuf {
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
