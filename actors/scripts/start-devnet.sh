#!/usr/bin/env bash

# Check if script is run from the 'actors' directory
if [[ "$(basename "$PWD")" != "actors" ]]; then
  echo "Error: This script must be run from the 'actors' directory." >&2
  exit 1
fi

# Source shared configuration
source "$(dirname "$0")/devnet-config.sh"

# Verify all actor binaries exist
for ENTRY in "${ACTORS[@]}"; do
  ACTOR=$(get_actor_name "$ENTRY")
  if [[ ! -x "$BUILD_DIR/$ACTOR" ]]; then
    echo "Error: Binary for '$ACTOR' not found at $BUILD_DIR/$ACTOR" >&2
    echo "Run ./compile-devnet.sh first." >&2
    exit 1
  fi
done

echo "Launching VMs for ${#ACTORS[@]} actors..."

# Launch a VM for each actor, copy binary, and start it
for ENTRY in "${ACTORS[@]}"; do
  ACTOR=$(get_actor_name "$ENTRY")
  ARGS=$(get_actor_args "$ENTRY")
  VM_NAME="mosaik-${ACTOR}"

  echo "==> Launching VM $VM_NAME for actor $ACTOR"
  multipass launch --name "$VM_NAME" --cpus 2 --memory 1G --disk 4G "$VM_IMAGE" || true
  multipass exec "$VM_NAME" -- sudo apt-get update -y
  multipass exec "$VM_NAME" -- sudo apt-get install -y daemonize 

  echo "==> Copying $ACTOR binary to $VM_NAME"
  multipass transfer "$BUILD_DIR/$ACTOR" "$VM_NAME:/home/ubuntu/$ACTOR"

  echo "==> Marking binary executable in $VM_NAME"
  multipass exec "$VM_NAME" -- chmod +x "/home/ubuntu/$ACTOR"

  echo "==> Starting $ACTOR on $VM_NAME${ARGS:+ with args: $ARGS}"
  multipass exec "$VM_NAME" -- daemonize \
    -c /home/ubuntu \
    -o "/home/ubuntu/$ACTOR.log" \
    -l "/home/ubuntu/$ACTOR.lock" \
    "/home/ubuntu/$ACTOR" $ARGS
done

echo "Done. Current VMs:"
multipass list