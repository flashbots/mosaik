//! Shell script generation for TDX guest launch.

use {
	super::helpers::ssh_fwd_rule,
	std::{
		fs::{self, File},
		io::Write,
		path::Path,
		process::Command,
	},
};

pub(super) fn generate_launch_script(
	crate_name: &str,
	artifacts_dir: &Path,
	cpus: u32,
	memory: &str,
	ssh_forward: Option<(u16, u16)>,
) {
	let ssh_fwd = ssh_fwd_rule(ssh_forward);
	let launch_script = include_str!("templates/launch-tdx.sh")
		.replace("{{CRATE_NAME}}", crate_name)
		.replace("{{CPUS}}", &cpus.to_string())
		.replace("{{MEMORY}}", memory)
		.replace("{{SSH_FWD}}", &ssh_fwd);

	let launch_script_path =
		artifacts_dir.join(format!("{crate_name}-launch-tdx.sh"));
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

#[expect(clippy::too_many_arguments)]
pub(super) fn generate_self_extracting_script(
	crate_name: &str,
	artifacts_dir: &Path,
	out_dir: &Path,
	kernel_output: &Path,
	final_path: &Path,
	ovmf_output: &Path,
	cpus: u32,
	memory: &str,
	ssh_forward: Option<(u16, u16)>,
) {
	let run_qemu_path = artifacts_dir.join(format!("{crate_name}-run-qemu.sh"));

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
		fs::copy(ovmf_output, bundle_dir.join("ovmf.fd")).unwrap();
	}

	let tar_path = out_dir.join("payload.tar.gz");
	let uncompressed_tar = out_dir.join("payload.tar");
	let mut tar_args = vec!["vmlinuz", "initramfs.cpio.gz"];
	if ovmf_output.exists() {
		tar_args.push("ovmf.fd");
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
	let header = include_str!("templates/run-qemu.sh")
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
