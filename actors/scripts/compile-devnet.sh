#!/bin/bash

# Check if script is run from the 'actors' directory
if [[ "$(basename "$PWD")" != "actors" ]]; then
  echo "Error: This script must be run from the 'actors' directory." >&2
  exit 1
fi

# Source shared configuration
source "$(dirname "$0")/devnet-config.sh"

# Check if build VM already exists
if multipass info mosaik-build &>/dev/null; then
  echo "==> Build VM 'mosaik-build' already exists, reusing it"
  # Ensure it's running
  multipass start mosaik-build 2>/dev/null || true
else
  echo "==> Creating build VM 'mosaik-build'"
  multipass launch --name mosaik-build --cpus 10 --memory 32G --disk 20G $VM_IMAGE
  multipass mount .. mosaik-build:/home/ubuntu/project

  # Install dependencies and set up build environment
  multipass exec mosaik-build -- bash -lc '
    cd /home/ubuntu
    sudo apt-get update -y
    sudo apt-get install -y build-essential clang pkg-config libssl-dev git ca-certificates

    # Install Rust
    if ! command -v cargo >/dev/null 2>&1; then
      curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
      echo "source \$HOME/.cargo/env" >> ~/.bashrc
    fi

    source $HOME/.cargo/env
    mkdir -p /home/ubuntu/build
  '
fi
mkdir -p ../target/devnet-build

# build each actor
for ENTRY in "${ACTORS[@]}"; do
  ACTOR=$(get_actor_name "$ENTRY")
  ARGS=$(get_actor_args "$ENTRY")
  echo "Building actor: $ACTOR"
  multipass exec mosaik-build -- bash -lc "
    source \$HOME/.cargo/env
    cd /home/ubuntu/project/
    CARGO_TARGET_DIR=/home/ubuntu/build cargo build --release -p $ACTOR
  "
  multipass transfer mosaik-build:/home/ubuntu/build/release/$ACTOR ../target/devnet-build/$ACTOR
done

#multipass stop mosaik-build