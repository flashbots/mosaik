#!/usr/bin/env bash
set -euo pipefail

# Check if script is run from the 'actors' directory
if [[ "$(basename "$PWD")" != "actors" ]]; then
  echo "Error: This script must be run from the 'actors' directory." >&2
  exit 1
fi

# Source shared configuration
source "$(dirname "$0")/devnet-config.sh"


# Cleanup function to kill background processes
cleanup() {
  echo -e "\n==> Stopping log watchers..."
  kill $(jobs -p) 2>/dev/null || true
  exit 0
}

trap cleanup SIGINT SIGTERM

echo "Watching logs for ${#ACTORS[@]} actors (Ctrl+C to stop)..."
echo ""

# Start tailing logs for each actor
for i in "${!ACTORS[@]}"; do
  ENTRY="${ACTORS[$i]}"
  ACTOR=$(get_actor_name "$ENTRY")
  VM_NAME="mosaik-${ACTOR}"

  multipass exec "$VM_NAME" -- tail -n 100 -f "/home/ubuntu/$ACTOR.log" 2>/dev/null | \
    sed -u "s/^/[${ACTOR}] /" &
done

# Wait for all background processes
wait