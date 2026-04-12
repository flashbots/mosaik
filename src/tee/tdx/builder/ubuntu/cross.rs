//! GNU cross-compilation for TDX guest binaries.

use std::{
	env,
	fmt::Write,
	fs,
	path::{Path, PathBuf},
	process::Command,
};

#[allow(clippy::too_many_lines)]
pub(super) fn cross_compile_gnu(
	crate_name: &str,
	profile: &str,
	target_dir: &Path,
) -> Vec<u8> {
	let gnu_target = "x86_64-unknown-linux-gnu";

	eprintln!("==> Cross-compiling {crate_name} for {gnu_target}...");

	let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
	let inner_target_dir = target_dir.join("tdx-gnu-build");

	let mut cmd =
		Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".to_string()));

	cmd
		.arg("build")
		.arg("--target")
		.arg(gnu_target)
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
		let _ = write!(
			flags,
			"--remap-path-prefix={}=/build",
			manifest_dir.display()
		);

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
			let _ = write!(
				flags,
				"\x1f--remap-path-prefix={home}/registry/src=/registry"
			);
			let _ =
				write!(flags, "\x1f--remap-path-prefix={home}/git/checkouts=/git");
		}

		// Remap the rustup toolchain sysroot
		if let Ok(sysroot) = {
			Command::new("rustc")
				.args(["--print", "sysroot"])
				.output()
				.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
		} && !sysroot.is_empty()
		{
			let _ = write!(flags, "\x1f--remap-path-prefix={sysroot}/lib=/rustlib");
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

	let gnu_linker_env = "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER";
	let gnu_cc_env = "CC_x86_64_unknown_linux_gnu";

	if env::var(gnu_linker_env).is_err() {
		let candidates = ["x86_64-linux-gnu-gcc", "x86_64-unknown-linux-gnu-gcc"];

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
			cmd.env(gnu_linker_env, linker);
			cmd.env(gnu_cc_env, linker);
		} else if cfg!(not(target_os = "linux")) {
			panic!(
				"\n═════════════════════════════════════════════════════════════════\\
				 nNo GNU cross-linker found in PATH.\n\nInstall one:\n\nmacOS:  brew \
				 install SergioBenitez/osxct/x86_64-unknown-linux-gnu\nNix: nix-shell \
				 -p pkgsCross.gnu64.stdenv.cc\n\nOr set the linker \
				 explicitly:\n\nexport \
				 CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc\\
				 n═════════════════════════════════════════════════════════════════\n"
			);
		}
	} else {
		eprintln!(
			"  [linker] using {gnu_linker_env}={}",
			env::var(gnu_linker_env).unwrap()
		);
		cmd.env(gnu_linker_env, env::var(gnu_linker_env).unwrap());
		if let Ok(cc) = env::var(gnu_cc_env) {
			cmd.env(gnu_cc_env, cc);
		}
	}

	eprintln!(
		"  $ cargo build --target {gnu_target} --target-dir {} {}",
		inner_target_dir.display(),
		if profile == "release" {
			"--release"
		} else {
			""
		}
	);

	let status = cmd
		.status()
		.expect("failed to invoke cargo for GNU cross-build");

	assert!(
		status.success(),
		"\n══════════════════════════════════════════════════════════════════\\
 			 nCross-compilation to {gnu_target} failed.\n\nCheck the compiler output \
		 above for details.\nIf the linker failed, ensure your GNU \
		 cross-toolchain is correct:\n\nexport \
		 CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc\\
 			 n══════════════════════════════════════════════════════════════════\n"
	);

	let gnu_binary = inner_target_dir
		.join(gnu_target)
		.join(profile)
		.join(crate_name);

	assert!(
		gnu_binary.exists(),
		"Cross-build succeeded but binary not found at: {}",
		gnu_binary.display()
	);

	println!("cargo:rerun-if-changed={}", gnu_binary.display());

	let binary_data = fs::read(&gnu_binary).expect("failed to read GNU binary");

	assert!(
		!(binary_data.len() < 4 || &binary_data[..4] != b"\x7fELF"),
		"Binary at {} is not a Linux ELF",
		gnu_binary.display()
	);

	eprintln!(
		"  [ok] {} ({:.1} MB)",
		gnu_binary.display(),
		binary_data.len() as f64 / 1_048_576.0
	);

	binary_data
}
