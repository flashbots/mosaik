# Development image: builds group-chat natively for the host platform and runs
# it directly in a minimal container.  Works on Apple Silicon and x86_64.
#
# For real SGX deployment use Dockerfile.gramine (requires x86_64 Linux host).
FROM rust:slim AS builder

WORKDIR /build
COPY . .

RUN cargo build --release -p group-chat

# ─── Runtime ─────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update -qq \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/group-chat /app/group-chat

ENTRYPOINT ["/app/group-chat"]
CMD ["--nickname", "node", "--room", "tee-test"]
