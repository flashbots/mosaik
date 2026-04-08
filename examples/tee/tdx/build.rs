//! build.rs — Produces a bootable initramfs containing this crate's binary as
//! `/init`, suitable for direct-boot TDX guests (`qemu -kernel ... -initrd
//! ...`).
//!
//! ## What it does
//!
//! 1. Invokes `cargo build --target x86_64-unknown-linux-musl` as a subprocess
//!    to cross-compile this crate into a static Linux binary — regardless of
//!    what the host platform or outer build target is.
//! 2. Downloads Alpine Linux minirootfs (cached across builds).
//! 3. Assembles a CPIO "newc" archive (zero dependencies, writer is inlined):
//!    /init              — shell wrapper: mounts, loads tdx_guest, exec's
//!    binary /bin/busybox       — from Alpine minirootfs /bin/sh            —
//!    symlink → busybox /usr/bin/<binary>  — your compiled static Rust binary
//!    /lib/...           — musl dynamic linker (for busybox) /lib/modules/... —
//!    (optional) tdx_guest.ko if provided
//! 4. gzip-compresses the CPIO into:
//!    target/{release,debug}/<binary_name>-initramfs.cpio.gz
//!
//! ## Configuration (env vars)
//!
//! | Variable | Default | Description |
//! |----------|---------|-------------|
//! | `TDX_IMAGE_SKIP`           | unset  | Set to `1` to skip image build entirely |
//! | `TDX_IMAGE_ALPINE_VERSION` | `3.21` | Alpine release branch |
//! | `TDX_IMAGE_ALPINE_MINOR`   | `0`    | Alpine point release |
//! | `TDX_IMAGE_EXTRA_FILES`    | unset  | Colon-separated `host:guest` pairs to bundle |
//! | `TDX_IMAGE_KERNEL_MODULES` | unset  | Colon-separated paths to `.ko` files to bundle |
//! | `TDX_IMAGE_KERNEL_MODULES_DIR` | unset | Path to `/lib/modules/<ver>` — auto-discovers TDX .ko files |
//! | `TDX_IMAGE_OVMF`           | unset  | Path to OVMF.fd to precompute MRTD |
//! | `TDX_IMAGE_KERNEL`         | unset  | Path to vmlinuz with TDX guest support |
//!
//! ## Kernel
//!
//! Alpine's stock kernels do NOT have `CONFIG_INTEL_TDX_GUEST=y`. You must
//! provide a TDX-enabled kernel via `TDX_IMAGE_KERNEL`. On an Ubuntu 24.04
//! TDX host (after running Canonical's `setup-tdx-host.sh`), use:
//!
//!   `TDX_IMAGE_KERNEL=/boot/vmlinuz-$(uname -r)`
//!
//! The build script copies this kernel to the output directory and generates
//! a `launch-tdx.sh` helper script.
//!
//! ## Compile-time access
//!
//! The absolute path to the initramfs is exposed as:
//!   `env!("TDX_INITRAMFS_PATH")`
//!
//! If `TDX_IMAGE_OVMF` is set, the expected MRTD (hex) is exposed as:
//!   `env!("TDX_EXPECTED_MRTD")`
//! and written to `target/{profile}/<crate>-mrtd.hex`.

use std::{
	env,
	fs::{self, File},
	io::{self, BufWriter, Write},
	path::{Path, PathBuf},
	process::Command,
};

// ===========================================================================
// CPIO "newc" format writer — zero external dependencies
// ===========================================================================

struct CpioWriter<W: Write> {
	inner: W,
	ino: u32,
	offset: usize,
}

impl<W: Write> CpioWriter<W> {
	fn new(writer: W) -> Self {
		Self {
			inner: writer,
			ino: 1,
			offset: 0,
		}
	}

	fn align4(&mut self) -> io::Result<()> {
		let pad = (4 - (self.offset % 4)) % 4;
		if pad > 0 {
			self.inner.write_all(&vec![0u8; pad])?;
			self.offset += pad;
		}
		Ok(())
	}

	fn write_entry(
		&mut self,
		name: &str,
		data: &[u8],
		mode: u32,
	) -> io::Result<()> {
		let namesize = name.len() + 1;
		let filesize = data.len();

		let header = format!(
			"070701{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:\
			 08X}{:08X}{:08X}",
			self.ino,
			mode,
			0u32,
			0u32, // ino, mode, uid, gid
			1u32,
			0u32,
			filesize,
			0u32, // nlink, mtime, filesize, devmajor
			0u32,
			0u32,
			0u32,
			namesize,
			0u32, // devminor, rdevmaj, rdevmin, namesize, check
		);
		debug_assert_eq!(header.len(), 110);

		self.inner.write_all(header.as_bytes())?;
		self.offset += 110;

		self.inner.write_all(name.as_bytes())?;
		self.inner.write_all(&[0])?;
		self.offset += namesize;
		self.align4()?;

		self.inner.write_all(data)?;
		self.offset += filesize;
		self.align4()?;

		self.ino += 1;
		Ok(())
	}

	fn add_file(
		&mut self,
		path: &str,
		data: &[u8],
		executable: bool,
	) -> io::Result<()> {
		self.write_entry(path, data, if executable { 0o100755 } else { 0o100644 })
	}

	fn add_dir(&mut self, path: &str) -> io::Result<()> {
		self.write_entry(path, &[], 0o40755)
	}

	fn add_symlink(&mut self, path: &str, target: &str) -> io::Result<()> {
		self.write_entry(path, target.as_bytes(), 0o120777)
	}

	fn finish(mut self) -> io::Result<W> {
		self.write_entry("TRAILER!!!", &[], 0)?;
		let pad = (512 - (self.offset % 512)) % 512;
		if pad > 0 {
			self.inner.write_all(&vec![0u8; pad])?;
		}
		Ok(self.inner)
	}
}

// ===========================================================================
// Helpers
// ===========================================================================

fn env_or(key: &str, default: &str) -> String {
	env::var(key).unwrap_or_else(|_| default.to_string())
}

fn download_cached(url: &str, cache_dir: &Path, filename: &str) -> PathBuf {
	let cached = cache_dir.join(filename);
	if cached.exists() {
		eprintln!("  [cached] {}", cached.display());
		return cached;
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

fn extract_file_from_tar_gz(archive: &Path, inner_path: &str) -> Vec<u8> {
	let output = Command::new("tar")
		.args(["xzf"])
		.arg(archive)
		.args(["--to-stdout", inner_path])
		.output()
		.expect("failed to run tar");

	if !output.status.success() {
		panic!(
			"tar: failed to extract {inner_path} from {}: {}",
			archive.display(),
			String::from_utf8_lossy(&output.stderr)
		);
	}
	output.stdout
}

fn list_tar_gz(archive: &Path) -> Vec<String> {
	let output = Command::new("tar")
		.args(["tzf"])
		.arg(archive)
		.output()
		.expect("failed to run tar");

	if !output.status.success() {
		panic!("tar: failed to list {}", archive.display());
	}

	String::from_utf8_lossy(&output.stdout)
		.lines()
		.map(|l| l.trim_start_matches("./").to_string())
		.filter(|l| !l.is_empty())
		.collect()
}

/// Find the first file in a directory whose name starts with a given prefix.
fn find_file_starting_with(dir: &Path, prefix: &str) -> Option<PathBuf> {
	fs::read_dir(dir)
		.ok()?
		.filter_map(|e| e.ok())
		.find(|e| {
			e.file_name()
				.to_str()
				.map(|n| n.starts_with(prefix))
				.unwrap_or(false)
		})
		.map(|e| e.path())
}

/// Recursively find a file by name fragment under a directory (via `find`
/// command).
fn find_file_recursive(dir: &Path, name_fragment: &str) -> Option<PathBuf> {
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

/// Detect whether the outer build is release or debug by inspecting OUT_DIR.
fn detect_profile() -> &'static str {
	let out_dir = env::var("OUT_DIR").unwrap();
	if out_dir.contains("/release/") || out_dir.contains("\\release\\") {
		"release"
	} else {
		"debug"
	}
}

/// Walk up from OUT_DIR to find the workspace target/ directory.
fn find_target_dir() -> PathBuf {
	if let Ok(d) = env::var("CARGO_TARGET_DIR") {
		return PathBuf::from(d);
	}

	let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
	let mut dir = out_dir.as_path();
	loop {
		if dir.file_name().map(|n| n == "target").unwrap_or(false) {
			return dir.to_path_buf();
		}
		match dir.parent() {
			Some(parent) if parent != dir => dir = parent,
			_ => {
				// Fallback: strip known suffix depth from OUT_DIR
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

// ===========================================================================
// Main
// ===========================================================================

fn main() {
	// --- Recursion guard ---------------------------------------------------
	// The inner `cargo build --target musl` will also run this build.rs.
	// Detect that and bail out immediately so we don't infinitely recurse.
	if env::var("__TDX_IMAGE_INNER_BUILD").is_ok() {
		return;
	}

	println!("cargo:rerun-if-env-changed=TDX_IMAGE_SKIP");
	println!("cargo:rerun-if-env-changed=TDX_IMAGE_ALPINE_VERSION");
	println!("cargo:rerun-if-env-changed=TDX_IMAGE_ALPINE_MINOR");
	println!("cargo:rerun-if-env-changed=TDX_IMAGE_EXTRA_FILES");
	println!("cargo:rerun-if-env-changed=TDX_IMAGE_KERNEL_MODULES");

	if env_or("TDX_IMAGE_SKIP", "0") == "1" {
		eprintln!("TDX_IMAGE_SKIP=1 — skipping initramfs build");
		return;
	}

	let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
	let cache_dir = out_dir.join("tdx-image-cache");
	fs::create_dir_all(&cache_dir).unwrap();

	let crate_name = env::var("CARGO_PKG_NAME").unwrap();
	let profile = detect_profile();
	let target_dir = find_target_dir();
	let alpine_ver = env_or("TDX_IMAGE_ALPINE_VERSION", "3.21");
	let alpine_minor = env_or("TDX_IMAGE_ALPINE_MINOR", "0");

	let output_filename = format!("{crate_name}-initramfs.cpio.gz");
	let final_path = target_dir.join(profile).join(&output_filename);

	eprintln!("==> TDX initramfs: profile={profile}, crate={crate_name}");
	eprintln!("    output = {}", final_path.display());

	// -------------------------------------------------------------------
	// 1. Cross-compile for x86_64-unknown-linux-musl
	// -------------------------------------------------------------------
	let musl_target = "x86_64-unknown-linux-musl";

	eprintln!("==> Cross-compiling {crate_name} for {musl_target}...");

	let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

	// Use a separate target directory for the inner build to avoid deadlocking
	// on the cargo artifact directory lock — the outer `cargo build` already
	// holds a lock on `target/`, so a subprocess writing to the same directory
	// would block forever.
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

	// Set the recursion guard so the inner build.rs is a no-op.
	cmd.env("__TDX_IMAGE_INNER_BUILD", "1");

	// On macOS (or any non-Linux host), the default `cc` is Apple Clang which
	// cannot link Linux/musl binaries.  We need to explicitly point cargo at a
	// musl cross-linker.  The env var CARGO_TARGET_<TRIPLE>_LINKER overrides
	// whatever is (or isn't) in .cargo/config.toml.
	//
	// We also set CC_x86_64_unknown_linux_musl so that C dependencies (ring,
	// blake3, etc.) compile with the correct cross-compiler.
	let musl_linker_env = "CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER";
	let musl_cc_env = "CC_x86_64_unknown_linux_musl";

	if env::var(musl_linker_env).is_err() {
		// Try to find a musl cross-gcc in PATH.
		let candidates = [
			"x86_64-linux-musl-gcc", // Homebrew filosottile/musl-cross
			"x86_64-linux-gnu-gcc",  // sometimes works with musl sysroot
			"musl-gcc",              // system musl-gcc wrapper (Linux)
		];

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
			// On non-Linux hosts without a cross-linker, fail early with a
			// helpful message instead of letting the build produce a cryptic
			// Apple ld error.
			panic!(
				"\n═══════════════════════════════════════════════════════════════\\
				 nNo musl cross-linker found in PATH.\n\nInstall one:\n\nmacOS:  brew \
				 install filosottile/musl-cross/musl-cross\nNix:    nix-shell -p \
				 pkgsCross.musl64.stdenv.cc\n\nOr set the linker \
				 explicitly:\n\nexport \
				 CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=x86_64-linux-musl-gcc\\
				 n═══════════════════════════════════════════════════════════════\n"
			);
		}
		// On Linux the default `cc` can usually link musl targets, so we
		// don't force a linker override.
	} else {
		eprintln!(
			"  [linker] using {musl_linker_env}={}",
			env::var(musl_linker_env).unwrap()
		);
		// Forward the user's explicit setting into the inner build.
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

	if !status.success() {
		panic!(
			"\n══════════════════════════════════════════════════════════════════\\
			 nCross-compilation to {musl_target} failed.\n\nCheck the compiler output \
			 above for details.\nIf the linker failed, ensure your musl \
			 cross-toolchain is correct:\n\nexport \
			 CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=x86_64-linux-musl-gcc\\
			 n══════════════════════════════════════════════════════════════════\n"
		);
	}

	let musl_binary = inner_target_dir
		.join(musl_target)
		.join(profile)
		.join(&crate_name);

	if !musl_binary.exists() {
		panic!(
			"Cross-build succeeded but binary not found at: {}",
			musl_binary.display()
		);
	}

	println!("cargo:rerun-if-changed={}", musl_binary.display());

	let binary_data = fs::read(&musl_binary).expect("failed to read musl binary");

	if binary_data.len() < 4 || &binary_data[..4] != b"\x7fELF" {
		panic!("Binary at {} is not a Linux ELF", musl_binary.display());
	}

	eprintln!(
		"  [ok] {} ({:.1} MB)",
		musl_binary.display(),
		binary_data.len() as f64 / 1_048_576.0
	);

	// -------------------------------------------------------------------
	// 2. Download Alpine minirootfs
	// -------------------------------------------------------------------
	eprintln!("==> Downloading Alpine minirootfs...");
	let alpine_tar_name =
		format!("alpine-minirootfs-{alpine_ver}.{alpine_minor}-x86_64.tar.gz");
	let alpine_url = format!(
        "https://dl-cdn.alpinelinux.org/alpine/v{alpine_ver}/releases/x86_64/{alpine_tar_name}"
    );
	let alpine_tar = download_cached(&alpine_url, &cache_dir, &alpine_tar_name);

	// -------------------------------------------------------------------
	// 3. Extract busybox + musl libs
	// -------------------------------------------------------------------
	eprintln!("==> Extracting busybox...");
	let busybox_data = extract_file_from_tar_gz(&alpine_tar, "./bin/busybox");
	if busybox_data.is_empty() {
		panic!("Failed to extract bin/busybox from Alpine minirootfs");
	}
	eprintln!(
		"  [ok] busybox ({:.0} KB)",
		busybox_data.len() as f64 / 1024.0
	);

	let tar_entries = list_tar_gz(&alpine_tar);
	let musl_libs: Vec<_> = tar_entries
		.iter()
		.filter(|e| e.starts_with("lib/ld-musl") || e.starts_with("lib/libc.musl"))
		.cloned()
		.collect();

	// -------------------------------------------------------------------
	// 4. Generate /init
	// -------------------------------------------------------------------
	eprintln!("==> Generating /init...");
	let init_script = format!(
		r#"#!/bin/sh
# TDX guest init — runs as PID 1.
# Mounts filesystems, loads tdx_guest driver, exec's the workload.

/bin/busybox mount -t proc     proc     /proc
/bin/busybox mount -t sysfs    sysfs    /sys
/bin/busybox mount -t devtmpfs devtmpfs /dev
/bin/busybox mkdir -p /dev/pts /dev/shm /tmp /run
/bin/busybox mount -t devpts   devpts   /dev/pts
/bin/busybox mount -t tmpfs    tmpfs    /tmp
/bin/busybox mount -t tmpfs    tmpfs    /run

# Provide all busybox applets
/bin/busybox --install -s /bin 2>/dev/null

# Load a kernel module by name.
# Tries modprobe first, then falls back to insmod with our bundled .ko files.
# Modules are pre-decompressed at build time — only plain .ko in /lib/modules/.
_load_mod() {{
    if modprobe "$1" 2>/dev/null; then
        echo "  [mod] $1: loaded via modprobe"
        return
    fi
    if [ -f /lib/modules/"$1".ko ]; then
        if insmod /lib/modules/"$1".ko 2>&1; then
            echo "  [mod] $1: loaded via insmod"
            return
        else
            echo "  [mod] $1: insmod FAILED (exit $?)"
            # Show the insmod error
            insmod /lib/modules/"$1".ko 2>&1 || true
            return
        fi
    fi
    echo "  [mod] $1: not found (built-in or missing)"
}}

# Load modules in dependency order.
# vsock must be up before tdx_guest registers its quote generation handler.

# 1. Core infrastructure
_load_mod configfs

# Mount configfs early — TSM needs it
mkdir -p /sys/kernel/config
mount -t configfs configfs /sys/kernel/config 2>/dev/null || true

# 2. TSM framework (must load before tdx_guest which depends on tsm_register)
_load_mod tsm

# 3. vsock transport (needed for quote generation via QGS on host)
_load_mod vsock
_load_mod vmw_vsock_virtio_transport_common
_load_mod vmw_vsock_virtio_transport

# 4. TDX guest driver (depends on tsm, registers as TSM provider)
#    Module is named tdx-guest.ko but the kernel sees it as tdx_guest
_load_mod tdx_guest
_load_mod tdx-guest

# Give vsock time to negotiate with the host QGS
sleep 1

# Verify TSM is available
if [ -d /sys/kernel/config/tsm ]; then
    echo "configfs-tsm: available at /sys/kernel/config/tsm"
else
    echo "configfs-tsm: not available (kernel may lack CONFIG_TSM_REPORTS)"
fi

# --- Diagnostics ---
echo "=== TDX Boot Diagnostics ==="

# Show what .ko files we actually bundled
echo "[bundled modules]"
ls -la /lib/modules/*.ko 2>/dev/null || echo "  (no .ko files in /lib/modules/)"

# Check which modules actually loaded
echo "[modules]"
cat /proc/modules 2>/dev/null | grep -E 'tdx|tsm|vsock|configfs' || echo "  (none of the expected modules found)"

# Check for TDX guest device
echo "[devices]"
ls -la /dev/tdx* 2>/dev/null || echo "  /dev/tdx_guest: not found"

# Check vsock
ls -la /dev/vsock 2>/dev/null || echo "  /dev/vsock: not found"
ls -la /dev/vhost-vsock 2>/dev/null || echo "  /dev/vhost-vsock: not found"

# Check configfs-tsm entries
echo "[configfs-tsm]"
ls -la /sys/kernel/config/tsm/ 2>/dev/null || echo "  /sys/kernel/config/tsm: empty or missing"
ls -la /sys/kernel/config/tsm/report/ 2>/dev/null || echo "  /sys/kernel/config/tsm/report: not found"

# Check kernel TDX support
echo "[kernel]"
dmesg 2>/dev/null | grep -iE 'tdx|tsm|vsock|quote' | tail -20 || true

echo "=== End Diagnostics ==="

# Bring up networking (QEMU user-mode gives us virtio eth0)
if [ -d /sys/class/net/eth0 ]; then
    ip link set lo up
    ip link set eth0 up
    udhcpc -i eth0 -s /bin/simple.script -q 2>/dev/null || true
fi

echo "=== TDX Guest Init ==="
echo "Starting workload: {name}"

# Replace shell with workload binary as PID 1.
# Binary MUST never return or the kernel panics.
exec /usr/bin/{name} "$@"
"#,
		name = crate_name
	);

	// -------------------------------------------------------------------
	// 4b. Obtain TDX-enabled kernel + modules (before CPIO assembly)
	// -------------------------------------------------------------------
	// We need the kernel modules in hand before building the CPIO so they
	// can be bundled into it. The vmlinuz itself is copied to the output
	// directory in step 8 (after the CPIO).
	//
	// Alpine kernels do NOT have CONFIG_INTEL_TDX_GUEST. We either use the
	// user's TDX_IMAGE_KERNEL or auto-download Ubuntu 24.04's generic kernel
	// from archive.ubuntu.com.

	println!("cargo:rerun-if-env-changed=TDX_IMAGE_KERNEL");
	println!("cargo:rerun-if-env-changed=TDX_IMAGE_KERNEL_VERSION");

	let kernel_cache_dir = cache_dir.join("kernel");
	fs::create_dir_all(&kernel_cache_dir).unwrap();

	// This will hold the path to a directory of extracted .ko files (if any)
	let mut auto_modules_dir: Option<PathBuf> = None;
	// This will hold the path to the vmlinuz to copy later
	let mut kernel_vmlinuz: Option<PathBuf> = None;

	if let Ok(kernel_path) = env::var("TDX_IMAGE_KERNEL") {
		// User provided an explicit kernel path — use it as-is.
		eprintln!("==> Using user-provided kernel: {kernel_path}");
		kernel_vmlinuz = Some(PathBuf::from(&kernel_path));
	} else {
		// Auto-download Ubuntu 24.04 generic kernel .deb from archive.ubuntu.com.
		// Ubuntu noble's generic kernel (6.8+) has CONFIG_INTEL_TDX_GUEST=y
		// built-in, plus CONFIG_TDX_GUEST_DRIVER=m and CONFIG_TSM_REPORTS=m as
		// modules.
		//
		// Pin with TDX_IMAGE_KERNEL_VERSION (e.g. "6.8.0-55").
		let kver = env_or("TDX_IMAGE_KERNEL_VERSION", "6.8.0-55");

		// Ubuntu package naming:
		// linux-image-unsigned-6.8.0-55-generic_6.8.0-55.57_amd64.deb The "57" is
		// the ABI/upload number — it increments with security patches.
		// We look for an already-cached version first, otherwise download.
		let kabi = env_or("TDX_IMAGE_KERNEL_ABI", "57");
		let image_deb_name =
			format!("linux-image-unsigned-{kver}-generic_{kver}.{kabi}_amd64.deb");
		// We need TWO module packages:
		//   linux-modules-*        → vsock.ko, configfs.ko,
		// vmw_vsock_virtio_transport*.ko   linux-modules-extra-*  → tdx_guest.ko,
		// tsm.ko
		let modules_base_deb_name =
			format!("linux-modules-{kver}-generic_{kver}.{kabi}_amd64.deb");
		let modules_extra_deb_name =
			format!("linux-modules-extra-{kver}-generic_{kver}.{kabi}_amd64.deb");

		let base_url = "https://archive.ubuntu.com/ubuntu/pool/main/l/linux";

		eprintln!("==> Auto-downloading Ubuntu {kver}-generic kernel...");
		eprintln!("    Set TDX_IMAGE_KERNEL=/path/to/vmlinuz to skip download");

		// Download the kernel image .deb
		let image_deb = download_cached(
			&format!("{base_url}/{image_deb_name}"),
			&kernel_cache_dir,
			&image_deb_name,
		);

		// Extract vmlinuz from the .deb (a .deb is an `ar` archive with data.tar.*
		// inside)
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

		// Extract data.tar.* contents
		if let Some(data_tar) = find_file_starting_with(&extract_dir, "data.tar") {
			let _ = Command::new("tar")
				.args(["xf"])
				.arg(&data_tar)
				.args(["-C"])
				.arg(&extract_dir)
				.status();
		}

		// Find vmlinuz
		let found_vmlinuz = find_file_recursive(&extract_dir, "vmlinuz");
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

		// Download and extract BOTH module .deb packages into a shared directory.
		// This gives us all the .ko files we need:
		//   linux-modules:       vsock.ko, configfs.ko,
		// vmw_vsock_virtio_transport*.ko   linux-modules-extra: tdx_guest.ko,
		// tsm.ko
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
				&kernel_cache_dir,
				deb_name,
			);

			// Extract .deb with `ar`, then unpack data.tar.*
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
					let _ = Command::new("tar")
						.args(["xf"])
						.arg(&data_tar)
						.args(["-C"])
						.arg(&modules_extract_dir)
						.status();
					eprintln!("  [ok] {label} extracted");
				} else {
					eprintln!("  [warn] no data.tar.* in {deb_name}");
				}
			} else {
				eprintln!("  [warn] ar failed on {deb_name}");
			}

			let _ = fs::remove_dir_all(&tmp_dir);
		}

		auto_modules_dir = Some(modules_extract_dir);
		// Note: we do NOT delete extract_dir yet — we need it for CPIO module
		// discovery. It gets cleaned up after the CPIO is finalized.
	}

	// -------------------------------------------------------------------
	// 5. Assemble CPIO
	// -------------------------------------------------------------------
	eprintln!("==> Assembling CPIO...");

	let cpio_path = out_dir.join("initramfs.cpio");
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
		let data = extract_file_from_tar_gz(&alpine_tar, &format!("./{lib_entry}"));
		if !data.is_empty() {
			eprintln!("  [lib] /{lib_entry}");
			cpio.add_file(lib_entry, &data, true).unwrap();
		}
	}

	cpio
		.add_file(&format!("usr/bin/{crate_name}"), &binary_data, true)
		.unwrap();

	// Optional: extra files
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

	// Optional: explicit kernel module paths
	// (TDX_IMAGE_KERNEL_MODULES="/path/a.ko:...")
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
			let data = fs::read(module_path)
				.unwrap_or_else(|e| panic!("Failed to read {module_path}: {e}"));
			cpio.add_file(&guest_path, &data, false).unwrap();
		}
	}

	// Auto-discover TDX kernel modules from:
	//   1. TDX_IMAGE_KERNEL_MODULES_DIR env var (explicit path)
	//   2. auto_modules_dir (from auto-downloaded kernel .deb in step 4b)
	println!("cargo:rerun-if-env-changed=TDX_IMAGE_KERNEL_MODULES_DIR");

	let effective_modules_dir = env::var("TDX_IMAGE_KERNEL_MODULES_DIR")
		.map(PathBuf::from)
		.ok()
		.or_else(|| auto_modules_dir.clone());

	if let Some(ref modules_dir) = effective_modules_dir {
		eprintln!(
			"==> Auto-discovering TDX kernel modules from {}...",
			modules_dir.display()
		);

		let wanted_modules = [
			"configfs", // configfs filesystem
			"tsm",      // TSM core (CONFIG_TSM_REPORTS)
			"tdx_guest", /* TDX guest driver (underscore
			             * variant) */
			"tdx-guest", // TDX guest driver (hyphen variant)
			"vsock",     // core vsock (CONFIG_VSOCKETS)
			"vmw_vsock_virtio_transport", /* virtio vsock transport (for QGS
			              * communication) */
			"vmw_vsock_virtio_transport_common", // shared code for virtio vsock
		];

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

				// Decompress at build time — Ubuntu ships .ko.zst compressed
				// modules, but busybox's insmod can only load raw .ko files.
				// We decompress here so the guest doesn't need zstd/xz.
				let (data, guest_name) = if ko_filename.ends_with(".zst") {
					let out = Command::new("zstd")
						.args(["-d", "-c"])
						.arg(&ko_path)
						.output()
						.expect(
							"failed to run zstd — install zstd to decompress kernel modules",
						);
					if !out.status.success() {
						panic!("zstd failed to decompress {ko_path}");
					}
					(
						out.stdout,
						ko_filename.strip_suffix(".zst").unwrap().to_string(),
					)
				} else if ko_filename.ends_with(".xz") {
					let out = Command::new("xz")
						.args(["-d", "-c"])
						.arg(&ko_path)
						.output()
						.expect(
							"failed to run xz — install xz to decompress kernel modules",
						);
					if !out.status.success() {
						panic!("xz failed to decompress {ko_path}");
					}
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
			} else {
				eprintln!("  [module] {module_name}.ko not found (may be built-in)");
			}
		}
	} else {
		eprintln!(
			"==> No kernel modules directory -- TDX modules will not be bundled."
		);
	}

	// Clean up the auto-downloaded modules extract directory
	if let Some(ref dir) = auto_modules_dir {
		let _ = fs::remove_dir_all(dir);
	}

	cpio
		.add_file("etc/passwd", b"root:x:0:0:root:/:/bin/sh\n", false)
		.unwrap();
	cpio.add_file("etc/group", b"root:x:0:\n", false).unwrap();

	// Minimal udhcpc script — busybox's DHCP client calls this on lease events
	cpio
		.add_file(
			"bin/simple.script",
			br#"#!/bin/sh
case "$1" in
    bound|renew)
        ip addr flush dev "$interface"
        ip addr add "$ip/$mask" dev "$interface"
        if [ -n "$router" ]; then
            ip route add default via "$router"
        fi
        if [ -n "$dns" ]; then
            : > /etc/resolv.conf
            for d in $dns; do
                echo "nameserver $d" >> /etc/resolv.conf
            done
        fi
        ;;
esac
"#,
			true,
		)
		.unwrap();

	// Empty resolv.conf — udhcpc will populate it
	cpio.add_file("etc/resolv.conf", b"", false).unwrap();

	let writer = cpio.finish().unwrap();
	writer.into_inner().unwrap().sync_all().unwrap();

	// -------------------------------------------------------------------
	// 6. gzip → final output
	// -------------------------------------------------------------------
	eprintln!("==> Compressing...");
	let gz_path = out_dir.join(&output_filename);

	let gz_file = File::create(&gz_path).unwrap();
	let status = Command::new("gzip")
		.args(["-9", "-c"])
		.arg(&cpio_path)
		.stdout(gz_file)
		.status()
		.expect("failed to run gzip — is it installed?");

	if !status.success() {
		panic!("gzip compression failed");
	}

	// Copy to target/<profile>/<name>-initramfs.cpio.gz
	fs::create_dir_all(final_path.parent().unwrap()).unwrap();
	fs::copy(&gz_path, &final_path).unwrap();

	// -------------------------------------------------------------------
	// 7. Report initramfs
	// -------------------------------------------------------------------
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

	// -------------------------------------------------------------------
	// 8. Copy kernel to output directory
	// -------------------------------------------------------------------
	let kernel_output = target_dir
		.join(profile)
		.join(format!("{crate_name}-vmlinuz"));

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

	// -------------------------------------------------------------------
	// 9. Generate launch-tdx.sh helper script
	// -------------------------------------------------------------------
	// All paths are relative to the script's own directory so the artifacts
	// can be copied to the TDX host and run from anywhere.

	let launch_script = format!(
		r#"#!/usr/bin/env bash
# Auto-generated by build.rs — launch {crate_name} as a TDX guest VM.
#
# Usage: sudo ./launch-tdx.sh
#
# All file paths default to the same directory as this script.
# Override with environment variables if needed.
#
# Prerequisites:
#   - TDX-enabled host (Ubuntu 24.04 + Canonical's setup-tdx-host.sh)
#   - OVMF.fd (default: /usr/share/ovmf/OVMF.fd or set $OVMF)
#   - Kernel + initramfs in the same directory as this script

set -euo pipefail

# Resolve the directory this script lives in (works even if called via symlink)
SCRIPT_DIR="$(cd "$(dirname "${{BASH_SOURCE[0]}}")" && pwd)"

OVMF="${{OVMF:-$SCRIPT_DIR/{crate_name}-OVMF.fd}}"
# Fall back to host system OVMF if bundled one doesn't exist
if [ ! -f "$OVMF" ]; then
    OVMF="/usr/share/ovmf/OVMF.fd"
fi
KERNEL="${{KERNEL:-$SCRIPT_DIR/{crate_name}-vmlinuz}}"
INITRD="${{INITRD:-$SCRIPT_DIR/{crate_name}-initramfs.cpio.gz}}"
MEMORY="${{MEMORY:-4G}}"
CPUS="${{CPUS:-4}}"
SSH_PORT="${{SSH_PORT:-10022}}"

# Validate files exist
for f in "$OVMF" "$KERNEL" "$INITRD"; do
    if [ ! -f "$f" ]; then
        echo "ERROR: File not found: $f" >&2
        echo "       (set the corresponding env var to override)" >&2
        exit 1
    fi
done

TDX_OBJECT='{{"qom-type":"tdx-guest","id":"tdx0","sept-ve-disable":true,"quote-generation-socket":{{"type":"vsock","cid":"2","port":"4050"}}}}'
CMDLINE="console=ttyS0 ip=dhcp"

echo "=== Launching TDX Guest ==="
echo "  OVMF:     $OVMF"
echo "  Kernel:   $KERNEL"
echo "  Initrd:   $INITRD"
echo "  Memory:   $MEMORY"
echo "  CPUs:     $CPUS"
echo "  SSH:      localhost:$SSH_PORT"
echo ""

exec qemu-system-x86_64 \
    -accel kvm \
    -cpu host,pmu=off \
    -smp "$CPUS" \
    -m "$MEMORY" \
    \
    -object "$TDX_OBJECT" \
    -machine q35,kernel-irqchip=split,confidential-guest-support=tdx0 \
    \
    -bios "$OVMF" \
    -kernel "$KERNEL" \
    -initrd "$INITRD" \
    -append "$CMDLINE" \
    \
    -netdev user,id=net0,hostfwd=tcp::"$SSH_PORT"-:22 \
    -device virtio-net-pci,netdev=net0 \
    -device vhost-vsock-pci,guest-cid=3 \
    \
    -nographic \
    -nodefaults \
    -serial mon:stdio
"#,
		crate_name = crate_name,
	);

	let launch_script_path = target_dir
		.join(profile)
		.join(format!("{crate_name}-launch-tdx.sh"));
	fs::write(&launch_script_path, &launch_script).unwrap();

	// Make it executable (Unix only)
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

	// -------------------------------------------------------------------
	// 10. Obtain OVMF.fd and precompute MRTD
	// -------------------------------------------------------------------
	// MRTD only depends on the OVMF.fd firmware binary — it does NOT include
	// the kernel, initrd, or rootfs (those go into RTMRs at runtime).
	//
	// Strategy:
	//   a) TDX_IMAGE_OVMF is set → use that file
	//   b) Auto-download from Canonical's TDX PPA (tdx-specific OVMF build)
	//      or from Ubuntu's main archive (standard OVMF with TDX support)
	//
	// IMPORTANT: The MRTD depends on the EXACT OVMF.fd binary. The precomputed
	// MRTD will only match a live TD if the same OVMF.fd is used to launch it.
	// Canonical's TDX build (/usr/share/ovmf/OVMF.fd on TDX hosts) differs
	// from Ubuntu's stock OVMF package.

	println!("cargo:rerun-if-env-changed=TDX_IMAGE_OVMF");
	println!("cargo:rerun-if-env-changed=TDX_IMAGE_OVMF_VERSION");

	let ovmf_output = target_dir
		.join(profile)
		.join(format!("{crate_name}-OVMF.fd"));

	let ovmf_data: Option<Vec<u8>> = if let Ok(ovmf_path) =
		env::var("TDX_IMAGE_OVMF")
	{
		// User provided an explicit OVMF path
		eprintln!("==> Using user-provided OVMF: {ovmf_path}");
		println!("cargo:rerun-if-changed={ovmf_path}");
		Some(
			fs::read(&ovmf_path).unwrap_or_else(|e| {
				panic!("Failed to read OVMF.fd at {ovmf_path}: {e}")
			}),
		)
	} else {
		// Auto-download OVMF .deb and extract OVMF.fd
		// Try Canonical's TDX PPA first, then fall back to Ubuntu's archive
		eprintln!("==> Auto-downloading OVMF.fd...");

		let ovmf_version = env_or("TDX_IMAGE_OVMF_VERSION", "2024.02-3+tdx1.0");

		// Canonical's TDX PPA: ovmf_2024.02-3+tdx1.0_all.deb
		// Ubuntu archive:      ovmf_2024.02-2ubuntu0.4_all.deb
		let ovmf_urls = if ovmf_version.contains("tdx") {
			vec![
                format!(
                    "https://ppa.launchpadcontent.net/kobuk-team/tdx-release/ubuntu/pool/main/e/edk2/ovmf_{ovmf_version}_all.deb"
                ),
            ]
		} else {
			vec![
                format!(
                    "https://archive.ubuntu.com/ubuntu/pool/main/e/edk2/ovmf_{ovmf_version}_all.deb"
                ),
            ]
		};

		let mut downloaded = None;
		let ovmf_deb_name = format!("ovmf_{ovmf_version}_all.deb");

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
			} else {
				let _ = fs::remove_file(&cached);
				eprintln!("  [warn] Failed to download from {url}");
			}
		}

		if let Some(deb_path) = downloaded {
			// Extract OVMF.fd from the .deb
			let ovmf_extract = kernel_cache_dir.join("extract-ovmf");
			let _ = fs::remove_dir_all(&ovmf_extract);
			fs::create_dir_all(&ovmf_extract).unwrap();

			let _ = Command::new("ar")
				.args(["x"])
				.arg(&deb_path)
				.current_dir(&ovmf_extract)
				.status();

			if let Some(data_tar) = find_file_starting_with(&ovmf_extract, "data.tar")
			{
				let _ = Command::new("tar")
					.args(["xf"])
					.arg(&data_tar)
					.args(["-C"])
					.arg(&ovmf_extract)
					.status();
			}

			// Look for OVMF.fd — it's at /usr/share/ovmf/OVMF.fd inside the .deb
			let found = find_file_recursive(&ovmf_extract, "OVMF.fd");
			let result = if let Some(ref ovmf_path) = found {
				let data = fs::read(ovmf_path).ok();
				if let Some(ref d) = data {
					eprintln!(
						"  [ok] OVMF.fd extracted ({:.1} MB)",
						d.len() as f64 / 1_048_576.0
					);
				}
				data
			} else {
				eprintln!("  [warn] OVMF.fd not found in .deb");
				None
			};

			let _ = fs::remove_dir_all(&ovmf_extract);
			result
		} else {
			eprintln!(
				"  [warn] Could not download OVMF .deb — skipping MRTD precomputation"
			);
			eprintln!(
				"         Set TDX_IMAGE_OVMF=/path/to/OVMF.fd to provide it manually"
			);
			None
		}
	};

	// Copy OVMF.fd to output directory and precompute MRTD
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
				let hex = digest
					.iter()
					.map(|b| format!("{b:02x}"))
					.collect::<String>();
				println!("cargo:warning=MRTD: {hex}");
				println!("cargo:rustc-env=TDX_EXPECTED_MRTD={hex}");

				let mrtd_path = target_dir
					.join(profile)
					.join(format!("{crate_name}-mrtd.hex"));
				fs::write(&mrtd_path, &hex).unwrap();
				println!("cargo:warning=MRTD written to: {}", mrtd_path.display());
			}
			Err(e) => {
				println!("cargo:warning=MRTD computation failed: {e}");
			}
		}
	}

	// -------------------------------------------------------------------
	// 11. Generate self-extracting run-qemu.sh
	// -------------------------------------------------------------------
	// Bundles vmlinuz + initramfs + OVMF.fd + launch logic into a single
	// file that can be scp'd to a TDX host and run directly:
	//
	//   scp target/release/<crate>-run-qemu.sh tdxhost:~/
	//   ssh tdxhost 'sudo ~/run-qemu.sh'
	//
	// The script extracts the payload to a temp dir, launches QEMU, and
	// cleans up on exit.

	let run_qemu_path = target_dir
		.join(profile)
		.join(format!("{crate_name}-run-qemu.sh"));

	if kernel_output.exists() && final_path.exists() {
		eprintln!("==> Generating self-extracting {crate_name}-run-qemu.sh...");

		// Create a tar archive of the three payload files in a temp dir
		let bundle_dir = out_dir.join("bundle");
		let _ = fs::remove_dir_all(&bundle_dir);
		fs::create_dir_all(&bundle_dir).unwrap();

		// Copy files with short names into bundle dir
		fs::copy(&kernel_output, bundle_dir.join("vmlinuz")).unwrap();
		fs::copy(&final_path, bundle_dir.join("initramfs.cpio.gz")).unwrap();
		if ovmf_output.exists() {
			fs::copy(&ovmf_output, bundle_dir.join("OVMF.fd")).unwrap();
		}

		// Create the tar payload
		let tar_path = out_dir.join("payload.tar.gz");
		let tar_status = Command::new("tar")
			.args(["czf"])
			.arg(&tar_path)
			.args(["-C"])
			.arg(&bundle_dir)
			.args(["vmlinuz", "initramfs.cpio.gz"])
			.args(if ovmf_output.exists() {
				vec!["OVMF.fd"]
			} else {
				vec![]
			})
			.status()
			.expect("failed to run tar");

		if !tar_status.success() {
			eprintln!("  [warn] Failed to create tar payload — skipping run-qemu.sh");
		} else {
			let payload = fs::read(&tar_path).unwrap();

			let header = format!(
				r#"#!/usr/bin/env bash
# Self-extracting TDX guest launcher for {crate_name}
# Generated by build.rs — contains vmlinuz + initramfs + OVMF.fd
#
# Usage:
#   sudo ./{crate_name}-run-qemu.sh
#
# Environment variables:
#   MEMORY    — guest RAM (default: 4G)
#   CPUS      — guest vCPUs (default: 4)
#   SSH_PORT  — host port forwarded to guest :22 (default: 10022)
#   OVMF      — override OVMF.fd path (default: extracted from this script)
#   KERNEL    — override vmlinuz path
#   INITRD    — override initramfs path
#   KEEP      — set to 1 to keep extracted files after exit

set -euo pipefail

MARKER="__PAYLOAD_BELOW__"
MEMORY="${{MEMORY:-4G}}"
CPUS="${{CPUS:-4}}"
SSH_PORT="${{SSH_PORT:-10022}}"

# Create temp dir and ensure cleanup
WORK_DIR=$(mktemp -d /tmp/{crate_name}-tdx.XXXXXX)
cleanup() {{
    if [ "${{KEEP:-0}}" = "1" ]; then
        echo "Extracted files kept at: $WORK_DIR"
    else
        rm -rf "$WORK_DIR"
    fi
}}
trap cleanup EXIT

# Extract payload
echo "Extracting TDX guest image..."
PAYLOAD_LINE=$(grep -an "^$MARKER" "$0" | tail -1 | cut -d: -f1)
PAYLOAD_START=$((PAYLOAD_LINE + 1))
tail -n +$PAYLOAD_START "$0" | tar xzf - -C "$WORK_DIR"

OVMF="${{OVMF:-$WORK_DIR/OVMF.fd}}"
KERNEL="${{KERNEL:-$WORK_DIR/vmlinuz}}"
INITRD="${{INITRD:-$WORK_DIR/initramfs.cpio.gz}}"

# Fall back to system OVMF if not bundled
if [ ! -f "$OVMF" ]; then
    OVMF="/usr/share/ovmf/OVMF.fd"
fi

for f in "$OVMF" "$KERNEL" "$INITRD"; do
    if [ ! -f "$f" ]; then
        echo "ERROR: Missing file: $f" >&2
        exit 1
    fi
done

TDX_OBJECT='{{"qom-type":"tdx-guest","id":"tdx0","sept-ve-disable":true,"quote-generation-socket":{{"type":"vsock","cid":"2","port":"4050"}}}}'
CMDLINE="console=ttyS0 ip=dhcp"

echo "=== Launching TDX Guest: {crate_name} ==="
echo "  OVMF:     $OVMF"
echo "  Kernel:   $KERNEL"
echo "  Initrd:   $INITRD"
echo "  Memory:   $MEMORY"
echo "  CPUs:     $CPUS"
echo "  SSH:      localhost:$SSH_PORT"
echo ""

exec qemu-system-x86_64 \
    -accel kvm \
    -cpu host,pmu=off \
    -smp "$CPUS" \
    -m "$MEMORY" \
    \
    -object "$TDX_OBJECT" \
    -machine q35,kernel-irqchip=split,confidential-guest-support=tdx0 \
    \
    -bios "$OVMF" \
    -kernel "$KERNEL" \
    -initrd "$INITRD" \
    -append "$CMDLINE" \
    \
    -netdev user,id=net0,hostfwd=tcp::"$SSH_PORT"-:22 \
    -device virtio-net-pci,netdev=net0 \
    -device vhost-vsock-pci,guest-cid=3 \
    \
    -nographic \
    -nodefaults \
    -serial mon:stdio

# The payload tar is appended below this marker — do not remove it
{marker}
"#,
				crate_name = crate_name,
				marker = "MARKER", // placeholder, replaced below
			);

			// Replace the "MARKER" placeholder with the actual marker string
			let header = header.replace(
				&format!("\n{}\n", "MARKER"),
				&format!("\n{}\n", "__PAYLOAD_BELOW__"),
			);

			// Write header + payload
			let mut out_file = File::create(&run_qemu_path).unwrap();
			out_file.write_all(header.as_bytes()).unwrap();
			out_file.write_all(&payload).unwrap();
			out_file.sync_all().unwrap();

			// Make executable
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
		}

		// Cleanup
		let _ = fs::remove_dir_all(&bundle_dir);
		let _ = fs::remove_file(out_dir.join("payload.tar.gz"));
	} else {
		eprintln!("==> Skipping run-qemu.sh (kernel or initramfs not available)");
	}
}

// ===========================================================================
// MRTD precomputation — replays the TDX module's measurement algorithm
// ===========================================================================
//
// References:
//   - Intel TDX Module 1.0 EAS, Chapter 10 (Measurement and Attestation)
//   - Intel TDX Virtual Firmware Design Guide (344991-004US), Chapter 8
//   - td-shim spec:
//     github.com/confidential-containers/td-shim/blob/main/doc/tdshim_spec.md
//   - TDVF binary layout: UEFI Forum presentation, Dec 2020
//
// Algorithm:
//   1. Parse TDVF_DESCRIPTOR from the end of the OVMF.fd binary
//   2. For each TDVF_SECTION with EXTENDMR attribute: a. For each 4KB page in
//      the section (lowest GPA → highest):
//         - Simulate TDH.MEM.PAGE.ADD: extend MRTD with SHA384 of a 128-byte
//           buffer containing "MEM.PAGE.ADD\0..." || GPA (le64)
//         - Simulate TDH.MR.EXTEND: for each 256-byte chunk in the page, extend
//           MRTD with SHA384 of (128-byte header || 256-byte chunk data)
//   3. Finalize → the resulting SHA-384 digest is the MRTD
//
// Since build.rs cannot use external crates, we shell out to `openssl` or
// use a minimal inlined SHA-384 implementation.

mod mrtd {
	use std::process::Command;

	const PAGE_SIZE: usize = 4096;
	const CHUNK_SIZE: usize = 256;
	const CHUNKS_PER_PAGE: usize = PAGE_SIZE / CHUNK_SIZE; // 16
	const SHA384_LEN: usize = 48;

	// TDVF metadata GUID: e47a6535-984a-4798-865e-4685a7bf8ec2
	const TDVF_METADATA_GUID: [u8; 16] = [
		0x35, 0x65, 0x7a, 0xe4, 0x4a, 0x98, 0x98, 0x47, 0x86, 0x5e, 0x46, 0x85,
		0xa7, 0xbf, 0x8e, 0xc2,
	];

	// Section types
	const SECTION_TYPE_BFV: u32 = 0;
	const SECTION_TYPE_CFV: u32 = 1;
	const SECTION_TYPE_TD_HOB: u32 = 2;
	const SECTION_TYPE_TEMP_MEM: u32 = 3;
	// Attribute flags
	const ATTR_MR_EXTEND: u32 = 0x00000001;
	const ATTR_PAGE_AUG: u32 = 0x00000002;

	#[derive(Debug)]
	struct TdvfSection {
		data_offset: u32,
		raw_data_size: u32,
		memory_address: u64,
		memory_data_size: u64,
		section_type: u32,
		attributes: u32,
	}

	/// Parse TDVF_DESCRIPTOR from the end of an OVMF.fd binary.
	///
	/// Layout (from the end of the file):
	///   offset (end - 0x20): 4-byte LE offset from end-of-file to
	/// TDVF_DESCRIPTOR   TDVF_DESCRIPTOR:
	///     [16 bytes] Signature GUID
	///     [4 bytes]  Length (of entire descriptor including sections)
	///     [4 bytes]  Version
	///     [4 bytes]  NumberOfSectionEntries
	///     [N × 32 bytes] TDVF_SECTION entries
	fn parse_tdvf_sections(ovmf: &[u8]) -> Result<Vec<TdvfSection>, String> {
		let len = ovmf.len();
		if len < 0x28 {
			return Err("OVMF.fd too small".into());
		}

		// The metadata offset pointer is at (end - 0x20)
		let offset_location = len - 0x20;
		let descriptor_offset = u32::from_le_bytes(
			ovmf[offset_location..offset_location + 4]
				.try_into()
				.unwrap(),
		) as usize;

		if descriptor_offset == 0 || descriptor_offset > len {
			return Err(format!(
				"Invalid TDVF descriptor offset: 0x{descriptor_offset:x} (file size: \
				 0x{len:x})"
			));
		}

		// The descriptor starts at (end - descriptor_offset)
		let desc_start = len - descriptor_offset;
		let desc = &ovmf[desc_start..];

		if desc.len() < 28 {
			return Err("TDVF descriptor region too small".into());
		}

		// Verify GUID signature
		let guid = &desc[0..16];
		if guid != TDVF_METADATA_GUID {
			return Err(format!("TDVF metadata GUID mismatch: got {:02x?}", guid));
		}

		let _length = u32::from_le_bytes(desc[16..20].try_into().unwrap());
		let version = u32::from_le_bytes(desc[20..24].try_into().unwrap());
		let num_sections = u32::from_le_bytes(desc[24..28].try_into().unwrap());

		eprintln!("  [tdvf] version={version}, sections={num_sections}");

		let mut sections = Vec::new();
		for i in 0..num_sections as usize {
			let base = 28 + i * 32;
			if base + 32 > desc.len() {
				return Err(format!("TDVF section {i} extends past descriptor"));
			}
			let s = &desc[base..base + 32];
			let section = TdvfSection {
				data_offset: u32::from_le_bytes(s[0..4].try_into().unwrap()),
				raw_data_size: u32::from_le_bytes(s[4..8].try_into().unwrap()),
				memory_address: u64::from_le_bytes(s[8..16].try_into().unwrap()),
				memory_data_size: u64::from_le_bytes(s[16..24].try_into().unwrap()),
				section_type: u32::from_le_bytes(s[24..28].try_into().unwrap()),
				attributes: u32::from_le_bytes(s[28..32].try_into().unwrap()),
			};
			let type_name = match section.section_type {
				SECTION_TYPE_BFV => "BFV",
				SECTION_TYPE_CFV => "CFV",
				SECTION_TYPE_TD_HOB => "TD_HOB",
				SECTION_TYPE_TEMP_MEM => "TEMP_MEM",
				_ => "UNKNOWN",
			};
			let mr = if section.attributes & ATTR_MR_EXTEND != 0 {
				" [EXTENDMR]"
			} else {
				""
			};
			let aug = if section.attributes & ATTR_PAGE_AUG != 0 {
				" [PAGE_AUG]"
			} else {
				""
			};
			eprintln!(
				"  [tdvf]   section[{i}]: type={type_name} gpa=0x{:x} size=0x{:x} \
				 raw_offset=0x{:x} raw_size=0x{:x}{mr}{aug}",
				section.memory_address,
				section.memory_data_size,
				section.data_offset,
				section.raw_data_size,
			);
			sections.push(section);
		}

		Ok(sections)
	}

	/// SHA-384 via openssl CLI — called for each extension step.
	/// This is slow but correct and has zero dependencies.
	fn sha384(data: &[u8]) -> Result<[u8; SHA384_LEN], String> {
		use std::{io::Write, process::Stdio};

		let mut child = Command::new("openssl")
			.args(["dgst", "-sha384", "-binary"])
			.stdin(Stdio::piped())
			.stdout(Stdio::piped())
			.stderr(Stdio::piped())
			.spawn()
			.map_err(|e| format!("failed to run openssl: {e}"))?;

		child
			.stdin
			.as_mut()
			.unwrap()
			.write_all(data)
			.map_err(|e| format!("failed to write to openssl stdin: {e}"))?;

		let output = child
			.wait_with_output()
			.map_err(|e| format!("openssl failed: {e}"))?;

		if !output.status.success() {
			return Err(format!(
				"openssl sha384 failed: {}",
				String::from_utf8_lossy(&output.stderr)
			));
		}

		if output.stdout.len() != SHA384_LEN {
			return Err(format!(
				"openssl sha384 returned {} bytes, expected {SHA384_LEN}",
				output.stdout.len()
			));
		}

		let mut hash = [0u8; SHA384_LEN];
		hash.copy_from_slice(&output.stdout);
		Ok(hash)
	}

	/// Compute the MRTD for a given OVMF.fd binary by replaying the
	/// TDX module's build-time measurement algorithm.
	pub fn compute_mrtd(ovmf: &[u8]) -> Result<[u8; SHA384_LEN], String> {
		let sections = parse_tdvf_sections(ovmf)?;

		// MRTD starts as all zeros
		let mut mrtd = [0u8; SHA384_LEN];

		// We process sections in order (index 0..N-1).
		// Only sections with ATTR_MR_EXTEND and without ATTR_PAGE_AUG are measured.
		for section in &sections {
			let extend_mr = section.attributes & ATTR_MR_EXTEND != 0;
			let page_aug = section.attributes & ATTR_PAGE_AUG != 0;

			if !extend_mr || page_aug {
				continue;
			}

			let gpa_base = section.memory_address;
			let mem_size = section.memory_data_size as usize;
			let raw_offset = section.data_offset as usize;
			let raw_size = section.raw_data_size as usize;

			// Get the section's data from the OVMF binary.
			// Pages beyond raw_data_size are zero-filled.
			let section_data = if raw_size > 0 && raw_offset + raw_size <= ovmf.len()
			{
				&ovmf[raw_offset..raw_offset + raw_size]
			} else {
				&[] as &[u8]
			};

			let num_pages = (mem_size + PAGE_SIZE - 1) / PAGE_SIZE;

			eprintln!(
				"  [mrtd] Measuring section at GPA 0x{gpa_base:x}, {num_pages} \
				 pages..."
			);

			for page_idx in 0..num_pages {
				let page_gpa = gpa_base + (page_idx as u64) * (PAGE_SIZE as u64);

				// --- TDH.MEM.PAGE.ADD ---
				// 128-byte buffer:
				//   [0..12]  = "MEM.PAGE.ADD"
				//   [12..16] = zeros
				//   [16..24] = GPA (little-endian u64)
				//   [24..128] = zeros
				let mut add_buf = [0u8; 128];
				add_buf[..12].copy_from_slice(b"MEM.PAGE.ADD");
				add_buf[16..24].copy_from_slice(&page_gpa.to_le_bytes());

				// MRTD = SHA384(MRTD || add_buf)
				let mut hash_input = Vec::with_capacity(SHA384_LEN + 128);
				hash_input.extend_from_slice(&mrtd);
				hash_input.extend_from_slice(&add_buf);
				mrtd = sha384(&hash_input)?;

				// --- TDH.MR.EXTEND for each 256-byte chunk ---
				// Get this page's data (zero-fill if beyond raw data)
				let page_data_start = page_idx * PAGE_SIZE;
				let mut page_data = [0u8; PAGE_SIZE];
				if page_data_start < section_data.len() {
					let available = section_data.len() - page_data_start;
					let copy_len = available.min(PAGE_SIZE);
					page_data[..copy_len].copy_from_slice(
						&section_data[page_data_start..page_data_start + copy_len],
					);
				}

				for chunk_idx in 0..CHUNKS_PER_PAGE {
					let chunk_gpa = page_gpa + (chunk_idx as u64) * (CHUNK_SIZE as u64);

					// First 128-byte extension buffer:
					//   [0..9]   = "MR.EXTEND"
					//   [9..16]  = zeros
					//   [16..24] = chunk GPA (little-endian u64)
					//   [24..128] = zeros
					let mut ext_header = [0u8; 128];
					ext_header[..9].copy_from_slice(b"MR.EXTEND");
					ext_header[16..24].copy_from_slice(&chunk_gpa.to_le_bytes());

					// The 256-byte chunk data (split into two 128-byte extension buffers
					// conceptually, but fed as one contiguous block to SHA384)
					let chunk_start = chunk_idx * CHUNK_SIZE;
					let chunk_data = &page_data[chunk_start..chunk_start + CHUNK_SIZE];

					// MRTD = SHA384(MRTD || ext_header || chunk_data)
					let mut hash_input =
						Vec::with_capacity(SHA384_LEN + 128 + CHUNK_SIZE);
					hash_input.extend_from_slice(&mrtd);
					hash_input.extend_from_slice(&ext_header);
					hash_input.extend_from_slice(chunk_data);
					mrtd = sha384(&hash_input)?;
				}
			}
		}

		let hex = mrtd.iter().map(|b| format!("{b:02x}")).collect::<String>();
		eprintln!("  [mrtd] MRTD = {hex}");

		Ok(mrtd)
	}
}
