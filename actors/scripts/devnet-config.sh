#!/bin/bash

# Actor definitions: "name:args" format (args are optional)
# Examples:
#   "canon"                     - no arguments
#   "canon:--port 8080"         - with arguments
#   "rpc:--rpc-url http://localhost:8545 --verbose"
ACTORS=(
  "beacon:-s c89efdaa54c0f20c7adf612882df0950f5a951637e0307cdcb4c672f298b8bc6"
  "canon:-p 272a994d8e20a665cac2967b740a9d549960e0d75eee02725cb7b9da351d9b55"
  "rpc:-p 272a994d8e20a665cac2967b740a9d549960e0d75eee02725cb7b9da351d9b55"
)

# VM settings
VM_IMAGE="resolute"
BUILD_DIR="../target/devnet-build"


# Helper function to extract actor name
get_actor_name() {
  echo "${1%%:*}"
}

# Helper function to extract actor args (empty if none)
get_actor_args() {
  local entry="$1"
  if [[ "$entry" == *":"* ]]; then
    echo "${entry#*:}"
  else
    echo ""
  fi
}