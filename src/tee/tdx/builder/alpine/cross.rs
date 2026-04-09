//! musl cross-compilation for TDX guest binaries.

use std::{
	env,
	fs,
	path::{Path, PathBuf},
	process::Command,
};

pub(super) fn cross_compile_musl(
	crate_name: &str,
	profile: &str,
	target_dir: &Path,
) -> Vec<u8> {
	let musl_target = "x86_64-unknown-linux-musl";

	eprintln!("==> Cross-compiling {crate_name} for {musl_target}...");

	let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
	let inner_target_dir = target_dir.join("tdx-musl-build");

	let mut cmd =
		Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".to_string()));

	cmd
		.arg("build")
		.arg("--target")
		.arg(musl_target)
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
		flags.push_str(&format!(
			"--remap-path-prefix={}=/build",
			manifest_dir.display()
		));

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
			flags.push_str(&format!(
				"\x1f--remap-path-prefix={home}/registry/src=/registry"
			));
			flags.push_str(&format!(
				"\x1f--remap-path-prefix={home}/git/checkouts=/git"
			));
		}

		// Remap the rustup toolchain sysroot
		if let Ok(sysroot) = {
			Command::new("rustc")
				.args(["--print", "sysroot"])
				.output()
				.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
		} {
			if !sysroot.is_empty() {
				flags
					.push_str(&format!("\x1f--remap-path-prefix={sysroot}/lib=/rustlib"));
			}
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

	let musl_linker_env = "CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER";
	let musl_cc_env = "CC_x86_64_unknown_linux_musl";

	if env::var(musl_linker_env).is_err() {
		let candidates =
			["x86_64-linux-musl-gcc", "x86_64-linux-gnu-gcc", "musl-gcc"];

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
			cmd.env(musl_linker_env, linker);
			cmd.env(musl_cc_env, linker);
		} else if cfg!(not(target_os = "linux")) {
			panic!(
				"\n═════════════════════════════════════════════════════════════════\\
				 nNo musl cross-linker found in PATH.\n\nInstall one:\n\nmacOS:  brew \
				 install filosottile/musl-cross/musl-cross\nNix: nix-shell -p \
				 pkgsCross.musl64.stdenv.cc\n\nOr set the linker \
				 explicitly:\n\nexport \
				 CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=x86_64-linux-musl-gcc\\
				 n═════════════════════════════════════════════════════════════════\n"
			);
		}
	} else {
		eprintln!(
			"  [linker] using {musl_linker_env}={}",
			env::var(musl_linker_env).unwrap()
		);
		cmd.env(musl_linker_env, env::var(musl_linker_env).unwrap());
		if let Ok(cc) = env::var(musl_cc_env) {
			cmd.env(musl_cc_env, cc);
		}
	}

	eprintln!(
		"  $ cargo build --target {musl_target} --target-dir {} {}",
		inner_target_dir.display(),
		if profile == "release" {
			"--release"
		} else {
			""
		}
	);

	let status = cmd
		.status()
		.expect("failed to invoke cargo for musl cross-build");

	assert!(
		status.success(),
		"\n══════════════════════════════════════════════════════════════════\\
 			 nCross-compilation to {musl_target} failed.\n\nCheck the compiler output \
		 above for details.\nIf the linker failed, ensure your musl \
		 cross-toolchain is correct:\n\nexport \
		 CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=x86_64-linux-musl-gcc\\
 			 n══════════════════════════════════════════════════════════════════\n"
	);

	let musl_binary = inner_target_dir
		.join(musl_target)
		.join(profile)
		.join(crate_name);

	assert!(
		musl_binary.exists(),
		"Cross-build succeeded but binary not found at: {}",
		musl_binary.display()
	);

	println!("cargo:rerun-if-changed={}", musl_binary.display());

	let binary_data = fs::read(&musl_binary).expect("failed to read musl binary");

	assert!(
		!(binary_data.len() < 4 || &binary_data[..4] != b"\x7fELF"),
		"Binary at {} is not a Linux ELF",
		musl_binary.display()
	);

	eprintln!(
		"  [ok] {} ({:.1} MB)",
		musl_binary.display(),
		binary_data.len() as f64 / 1_048_576.0
	);

	binary_data
}
