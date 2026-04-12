#!/bin/sh
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
_load_mod() {
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
            insmod /lib/modules/"$1".ko 2>&1 || true
            return
        fi
    fi
    echo "  [mod] $1: not found (built-in or missing)"
}

# Load modules in dependency order.
_load_mod configfs

mkdir -p /sys/kernel/config
mount -t configfs configfs /sys/kernel/config 2>/dev/null || true

_load_mod tsm
_load_mod vsock
_load_mod vmw_vsock_virtio_transport_common
_load_mod vmw_vsock_virtio_transport
_load_mod tdx_guest
_load_mod tdx-guest

sleep 1

if [ -d /sys/kernel/config/tsm ]; then
    echo "configfs-tsm: available at /sys/kernel/config/tsm"
else
    echo "configfs-tsm: not available (kernel may lack CONFIG_TSM_REPORTS)"
fi

# --- Diagnostics ---
echo "=== TDX Boot Diagnostics ==="

echo "[bundled modules]"
ls -la /lib/modules/*.ko 2>/dev/null || echo "  (no .ko files in /lib/modules/)"

echo "[modules]"
cat /proc/modules 2>/dev/null | grep -E 'tdx|tsm|vsock|configfs' || echo "  (none of the expected modules found)"

echo "[devices]"
ls -la /dev/tdx* 2>/dev/null || echo "  /dev/tdx_guest: not found"
ls -la /dev/vsock 2>/dev/null || echo "  /dev/vsock: not found"
ls -la /dev/vhost-vsock 2>/dev/null || echo "  /dev/vhost-vsock: not found"

echo "[configfs-tsm]"
ls -la /sys/kernel/config/tsm/ 2>/dev/null || echo "  /sys/kernel/config/tsm: empty or missing"
ls -la /sys/kernel/config/tsm/report/ 2>/dev/null || echo "  /sys/kernel/config/tsm/report: not found"

echo "[kernel]"
dmesg 2>/dev/null | grep -iE 'tdx|tsm|vsock|quote' | tail -20 || true

echo "=== End Diagnostics ==="

# Bring up networking
if [ -d /sys/class/net/eth0 ]; then
    ip link set lo up
    ip link set eth0 up
    udhcpc -i eth0 -s /bin/simple.script -q 2>/dev/null || true
fi
{{SSH_BLOCK}}
echo "=== TDX Guest Init ==="
echo "Starting workload: {{CRATE_NAME}}"
{{ENV_BLOCK}}
exec /usr/bin/{{CRATE_NAME}} {{ARGS}}
