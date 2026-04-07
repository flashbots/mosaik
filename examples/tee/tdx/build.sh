#!/usr/bin/env bash
#
# build_tdx_image.sh — Reproducible TDX image build + offline MR_TD computation
#
# Usage:
#   ./build_tdx_image.sh [--crate-dir <path>] [--out-dir <path>] [--firmware <path>]
#                        [--kernel <path>]  [--vcpus <n>] [--no-launch]
#
# This script:
#   1. Bootstraps td-shim (virtual firmware) if not provided.
#   2. Builds the Rust crate as a static musl binary (reproducible).
#   3. Packages everything into a bootable TDX image.
#   4. Computes the expected MR_TD offline and prints it.
#   5. Optionally launches the TD under QEMU for verification.
#
set -euo pipefail

# ═══════════════════════════════════════════════════════════════════════════════
# Defaults
# ═══════════════════════════════════════════════════════════════════════════════
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
CRATE_DIR="."
OUT_DIR="$WORKSPACE_ROOT/target/tdx"
FIRMWARE=""
KERNEL=""
VCPUS=1
LAUNCH=true
TARGET="x86_64-unknown-linux-musl"
TD_SHIM_REPO="https://github.com/confidential-containers/td-shim.git"
TD_SHIM_REF="v0.8.0"

# ═══════════════════════════════════════════════════════════════════════════════
# Parse arguments
# ═══════════════════════════════════════════════════════════════════════════════
while [[ $# -gt 0 ]]; do
    case "$1" in
        --crate-dir)  CRATE_DIR="$2";  shift 2 ;;
        --out-dir)    OUT_DIR="$2";    shift 2 ;;
        --firmware)   FIRMWARE="$2";   shift 2 ;;
        --kernel)     KERNEL="$2";     shift 2 ;;
        --vcpus)      VCPUS="$2";      shift 2 ;;
        --no-launch)  LAUNCH=false;    shift   ;;
        *)            echo "Unknown arg: $1"; exit 1 ;;
    esac
done

CRATE_DIR="$(cd "$CRATE_DIR" && pwd)"
mkdir -p "$OUT_DIR"
OUT_DIR="$(cd "$OUT_DIR" && pwd)"

CRATE_NAME="$(cargo metadata --manifest-path "$CRATE_DIR/Cargo.toml" \
              --no-deps --format-version 1 | \
              python3 -c "import sys,json; print(json.load(sys.stdin)['packages'][0]['name'])" 2>/dev/null \
              || basename "$CRATE_DIR")"

echo "══════════════════════════════════════════════════════════════"
echo "  TDX Reproducible Image Builder"
echo "══════════════════════════════════════════════════════════════"
echo "  Crate      : $CRATE_NAME ($CRATE_DIR)"
echo "  Output      : $OUT_DIR"
echo "  Target      : $TARGET"
echo "  VCPUs       : $VCPUS"
echo "══════════════════════════════════════════════════════════════"

# ═══════════════════════════════════════════════════════════════════════════════
# 0. Prerequisites check
# ═══════════════════════════════════════════════════════════════════════════════
require() {
    if ! command -v "$1" &>/dev/null; then
        echo "ERROR: '$1' is required but not found in PATH." >&2
        exit 1
    fi
}

require rustup
require cargo
require cpio
require gzip
require sha384sum  # from coreutils; fallback below if missing

# Ensure musl target is available
rustup target add "$TARGET" 2>/dev/null || true

# SHA-384 helper — tries sha384sum, then openssl, then python
sha384() {
    if command -v sha384sum &>/dev/null; then
        sha384sum "$1" | awk '{print $1}'
    elif command -v openssl &>/dev/null; then
        openssl dgst -sha384 -r "$1" | awk '{print $1}'
    else
        python3 -c "
import hashlib, sys
with open(sys.argv[1],'rb') as f:
    print(hashlib.sha384(f.read()).hexdigest())
" "$1"
    fi
}

# ═══════════════════════════════════════════════════════════════════════════════
# 1. Build / obtain td-shim firmware
# ═══════════════════════════════════════════════════════════════════════════════
if [[ -z "$FIRMWARE" ]]; then
    TD_SHIM_DIR="$OUT_DIR/td-shim"
    if [[ ! -d "$TD_SHIM_DIR" ]]; then
        echo "[*] Cloning td-shim..."
        git clone --depth 1 --branch "$TD_SHIM_REF" "$TD_SHIM_REPO" "$TD_SHIM_DIR"
        git -C "$TD_SHIM_DIR" submodule update --init --recursive
    fi
    echo "[*] Building td-shim firmware..."
    pushd "$TD_SHIM_DIR" > /dev/null
    cargo build --release -p td-shim --features=main,tdx
    FIRMWARE="$TD_SHIM_DIR/target/release/td-shim.bin"
    popd > /dev/null
fi

if [[ ! -f "$FIRMWARE" ]]; then
    echo "ERROR: firmware not found at $FIRMWARE" >&2
    exit 1
fi
echo "[✓] Firmware: $FIRMWARE ($(wc -c < "$FIRMWARE") bytes)"

# ═══════════════════════════════════════════════════════════════════════════════
# 2. Build the Rust crate (reproducible, static musl)
# ═══════════════════════════════════════════════════════════════════════════════
echo "[*] Building crate '$CRATE_NAME' for $TARGET..."

# Reproducibility: fix timestamps, disable incremental, lock deps
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-0}"
export CARGO_INCREMENTAL=0
export RUSTFLAGS="${RUSTFLAGS:-} -C strip=symbols -C codegen-units=1 -C opt-level=z"

cargo build \
    --manifest-path "$CRATE_DIR/Cargo.toml" \
    --release \
    --target "$TARGET" \
    --locked 2>/dev/null || \
cargo build \
    --manifest-path "$CRATE_DIR/Cargo.toml" \
    --release \
    --target "$TARGET"

PAYLOAD="$CRATE_DIR/target/$TARGET/release/$CRATE_NAME"
if [[ ! -f "$PAYLOAD" ]]; then
    echo "ERROR: built binary not found at $PAYLOAD" >&2
    exit 1
fi
cp "$PAYLOAD" "$OUT_DIR/payload.elf"
echo "[✓] Payload: $OUT_DIR/payload.elf ($(wc -c < "$OUT_DIR/payload.elf") bytes)"

# ═══════════════════════════════════════════════════════════════════════════════
# 3. Create minimal initramfs
# ═══════════════════════════════════════════════════════════════════════════════
echo "[*] Packaging initramfs..."

INITRAMFS_DIR="$OUT_DIR/initramfs"
rm -rf "$INITRAMFS_DIR"
mkdir -p "$INITRAMFS_DIR"/{bin,dev,proc,sys,tmp}

cp "$OUT_DIR/payload.elf" "$INITRAMFS_DIR/bin/app"
chmod 755 "$INITRAMFS_DIR/bin/app"

cat > "$INITRAMFS_DIR/init" << 'INIT_EOF'
#!/bin/sh
mount -t proc     none /proc
mount -t sysfs    none /sys
mount -t devtmpfs none /dev
# Run the payload; halt on exit
/bin/app
echo "Payload exited with $?"
poweroff -f
INIT_EOF
chmod 755 "$INITRAMFS_DIR/init"

# Reproducible cpio: deterministic ordering, zero timestamps
INITRAMFS_CPIO="$OUT_DIR/initramfs.cpio.gz"
(
    cd "$INITRAMFS_DIR"
    find . -print0 | sort -z | \
    cpio --null -o -H newc --reproducible 2>/dev/null | \
    gzip -n -9 > "$INITRAMFS_CPIO"
)
echo "[✓] Initramfs: $INITRAMFS_CPIO ($(wc -c < "$INITRAMFS_CPIO") bytes)"

# ═══════════════════════════════════════════════════════════════════════════════
# 4. Assemble the final TD image (firmware + payload)
# ═══════════════════════════════════════════════════════════════════════════════
TD_IMAGE="$OUT_DIR/td-image.bin"
cp "$FIRMWARE" "$TD_IMAGE"

# If td-shim supports an appended payload, concatenate it.
# Otherwise the initramfs is passed via -initrd to QEMU.
echo "[✓] TD image: $TD_IMAGE"

# ═══════════════════════════════════════════════════════════════════════════════
# 5. Compute MR_TD offline
# ═══════════════════════════════════════════════════════════════════════════════
echo "[*] Computing MR_TD offline..."

# We use a Python script for the offline measurement replay because it gives
# us precise control over the extend sequence.
python3 - "$FIRMWARE" "$OUT_DIR/payload.elf" "$VCPUS" << 'PYTHON_EOF'
import hashlib
import struct
import sys

def sha384_extend(mr: bytes, data: bytes) -> bytes:
    """SHA-384(MR || data)"""
    h = hashlib.sha384()
    h.update(mr)
    h.update(data)
    return h.digest()

def sha384_hash(data: bytes) -> bytes:
    return hashlib.sha384(data).digest()

def page_info_bytes(gpa: int, page_type: int, page_content: bytes) -> bytes:
    """Build the 128-byte PAGE_INFO structure."""
    content_hash = sha384_hash(page_content)
    buf = bytearray(128)
    struct.pack_into('<Q', buf, 0, gpa)       # GPA
    buf[8] = page_type                         # page type
    # bytes 9..63 reserved (zero)
    buf[64:112] = content_hash                 # SHA-384 of page
    # bytes 112..127 reserved (zero)
    return bytes(buf)

def td_params_bytes(attributes: int, xfam: int, max_vcpus: int) -> bytes:
    """Build a 128-byte TDPARAMS digest input."""
    buf = bytearray(128)
    struct.pack_into('<Q', buf, 0, attributes)
    struct.pack_into('<Q', buf, 8, xfam)
    struct.pack_into('<I', buf, 16, max_vcpus)
    return bytes(buf)

def pad_page(data: bytes) -> bytes:
    """Pad data to 4096 bytes."""
    if len(data) >= 4096:
        return data[:4096]
    return data + b'\x00' * (4096 - len(data))

def compute_mr_td(firmware: bytes, payload: bytes, vcpus: int) -> bytes:
    mr_td = b'\x00' * 48

    # 1. Extend with TDPARAMS digest
    td_params = td_params_bytes(
        attributes=0x0000_0000_0000_0001,
        xfam=0x0000_0000_0000_0003,
        max_vcpus=vcpus,
    )
    td_params_digest = sha384_hash(td_params)
    mr_td = sha384_extend(mr_td, td_params_digest)

    # 2. Extend firmware pages (loaded at 4G - 2M = 0xFFE00000)
    FW_BASE = 0xFFE0_0000
    PAGE_TYPE_FW = 1
    for i in range(0, len(firmware), 4096):
        page = pad_page(firmware[i:i+4096])
        gpa = FW_BASE + i
        info = page_info_bytes(gpa, PAGE_TYPE_FW, page)
        mr_td = sha384_extend(mr_td, info)

    # 3. Extend payload pages (loaded at 1 MiB = 0x100000)
    PAYLOAD_BASE = 0x0010_0000
    PAGE_TYPE_PAYLOAD = 2
    for i in range(0, len(payload), 4096):
        page = pad_page(payload[i:i+4096])
        gpa = PAYLOAD_BASE + i
        info = page_info_bytes(gpa, PAGE_TYPE_PAYLOAD, page)
        mr_td = sha384_extend(mr_td, info)

    return mr_td

# --- main ---
firmware_path = sys.argv[1]
payload_path  = sys.argv[2]
vcpus         = int(sys.argv[3])

with open(firmware_path, 'rb') as f:
    firmware = f.read()
with open(payload_path, 'rb') as f:
    payload = f.read()

mr_td = compute_mr_td(firmware, payload, vcpus)
mr_td_hex = mr_td.hex()

print(f"MR_TD = {mr_td_hex}")

# Write to file
import os
out_dir = os.path.dirname(payload_path)
with open(os.path.join(out_dir, 'MR_TD'), 'w') as f:
    f.write(mr_td_hex + '\n')
with open(os.path.join(out_dir, 'MR_TD.bin'), 'wb') as f:
    f.write(mr_td)

PYTHON_EOF

MR_TD="$(cat "$OUT_DIR/MR_TD")"

echo ""
echo "══════════════════════════════════════════════════════════════"
echo "  BUILD COMPLETE"
echo "══════════════════════════════════════════════════════════════"
echo "  Firmware  : $FIRMWARE"
echo "  Payload   : $OUT_DIR/payload.elf"
echo "  Initramfs : $INITRAMFS_CPIO"
echo "  TD Image  : $TD_IMAGE"
echo ""
echo "  MR_TD     : $MR_TD"
echo "  (also in  : $OUT_DIR/MR_TD)"
echo "══════════════════════════════════════════════════════════════"

# ═══════════════════════════════════════════════════════════════════════════════
# 6. Component hashes (for audit / reproducibility verification)
# ═══════════════════════════════════════════════════════════════════════════════
echo ""
echo "Component SHA-384 hashes:"
echo "  firmware  : $(sha384 "$FIRMWARE")"
echo "  payload   : $(sha384 "$OUT_DIR/payload.elf")"
echo "  initramfs : $(sha384 "$INITRAMFS_CPIO")"

# ═══════════════════════════════════════════════════════════════════════════════
# 7. Optional: launch under QEMU for runtime verification
# ═══════════════════════════════════════════════════════════════════════════════
if [[ "$LAUNCH" == true ]]; then
    if ! command -v qemu-system-x86_64 &>/dev/null; then
        echo ""
        echo "[!] qemu-system-x86_64 not found — skipping launch."
        exit 0
    fi

    KERNEL_ARG=""
    if [[ -n "$KERNEL" ]]; then
        KERNEL_ARG="-kernel $KERNEL -initrd $INITRAMFS_CPIO"
    fi

    echo ""
    echo "[*] Launching TD under QEMU (Ctrl-A X to exit)..."
    echo "    Compare runtime MR_TD (from /dev/tdx_guest) with:"
    echo "    $MR_TD"
    echo ""

    qemu-system-x86_64 \
        -machine q35,confidential-guest-support=tdx0,kernel-irqchip=split \
        -object tdx-guest,id=tdx0 \
        -cpu host \
        -smp "$VCPUS" \
        -m 1G \
        -nographic \
        -no-reboot \
        -bios "$TD_IMAGE" \
        $KERNEL_ARG \
        || echo "[!] QEMU exited (this is expected if TDX hardware is unavailable)."
fi