#!/usr/bin/env bash
set -euo pipefail

# Check if script is run from the 'actors' directory
if [[ "$(basename "$PWD")" != "actors" ]]; then
  echo "Error: This script must be run from the 'actors' directory." >&2
  exit 1
fi

# Source shared configuration
source "$(dirname "$0")/devnet-config.sh"

echo "Stopping and deleting ${#ACTORS[@]} actor VMs..."

for ENTRY in "${ACTORS[@]}"; do
  ACTOR=$(get_actor_name "$ENTRY")
  VM_NAME="mosaik-${ACTOR}"

  echo "==> Stopping and deleting VM $VM_NAME"
  multipass stop "$VM_NAME" 2>/dev/null || true
  multipass delete "$VM_NAME" 2>/dev/null || true
done

# Also stop the build VM if it exists
# echo "==> Stopping and deleting build VM mosaik-build"
# multipass stop mosaik-build 2>/dev/null || true
# multipass delete mosaik-build 2>/dev/null || true

echo "==> Purging deleted VMs"
multipass purge

echo "Done. Remaining VMs:"
multipass list