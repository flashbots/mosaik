# TEE Test Environment

Runs `group-chat` nodes in isolated containers that approximate TEE deployment.

## Files

| File | Purpose |
|------|---------|
| `Dockerfile` | Dev image: native build, runs the binary directly |
| `Dockerfile.gramine` | Production image: cross-compiles to x86_64, wraps in Gramine LibOS |
| `docker-compose.yml` | Three nodes: Alice, Bob, Charlie |
| `group-chat.manifest.template` | Gramine LibOS policy (filesystem allow-list, SGX settings) |

`Dockerfile.gramine` requires a native **x86_64 Linux** host.
`gramine-direct` (simulation mode) does not run under QEMU on Apple Silicon.

## Usage

```bash
# Build and start all three nodes
docker compose -f tests/tee/docker-compose.yml up --build -d

# Watch logs from all nodes
docker compose -f tests/tee/docker-compose.yml logs -f

# Send a message as Alice (Ctrl-P Ctrl-Q to detach without stopping)
docker attach tee-alice

# Check container status
docker compose -f tests/tee/docker-compose.yml ps

# Tear down
docker compose -f tests/tee/docker-compose.yml down
```

## Debugging discovery

If nodes aren't finding each other, run with debug logging:

```bash
RUST_LOG=mosaik=debug docker compose -f tests/tee/docker-compose.yml up
```

## Upgrading to real SGX

1. Use `Dockerfile.gramine` on an x86_64 Linux host with SGX support
2. Set `sgx.debug = false` in `group-chat.manifest.template`
3. Sign the manifest: `gramine-sgx-sign --key <key.pem> --manifest group-chat.manifest`
4. Change `ENTRYPOINT` in `Dockerfile.gramine` from `gramine-direct` to `gramine-sgx`
