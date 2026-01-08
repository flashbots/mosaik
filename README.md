# Mosaik — Distributed Order Pool Fabric
Mosaik is an experimental builder platform that coordinates a distributed order pool across specialized “actor” binaries built around a shared Rust crate. The repository hosts core state-management logic, integration scenarios, and orchestration binaries that mirror the topology used in production devnets.

## Status & Scope
- **Maturity:** Active research project; interfaces may change quickly.
- **Roadmap:** Track open issues and RFCs on GitHub; file new proposals before changing actor protocols or wire formats.
- **Contact:** Use repo issues/discussions; tag relevant actor owners for cross-component changes.

## Architecture
Actors communicate in a topology where the RPC entrypoint feeds `pending`, `builder`, and `canon` services, while `gc`, `reputation`, `sim`, `engine-api`, and `beacon` provide supporting roles. See `actors/README.md` for the current Mermaid diagram detailing message flow.

## Repository Layout
| Path         | Purpose                                                                                                             |
| ------------ | ------------------------------------------------------------------------------------------------------------------- |
| `src/`       | Shared library crate consumed by every actor binary.                                                                |
| `actors/*`   | Individual binary crates (`builder`, `rpc`, `canon`, `gc`, `pending`, `reputation`, `sim`, `engine-api`, `beacon`). |
| `tests/`     | Integration scenarios (`pubsub`, `discovery`, …) that exercise cross-actor flows.                                   |
| `docs/book/` | mdBook with long-form protocol documentation.                                                                       |
| `build/`     | Devnet tooling scripts (e.g., `just` recipes, multipass helpers).                                                   |

## Prerequisites & Setup
1. Install Rust toolchain ≥1.87 with `rustup toolchain install 1.87`.
2. Recommended: `cargo install just mdbook`.
3. Devnet control scripts expect Canonical Ubuntu VMs via Multipass and access to the published bootstrap peer key (`272a…d9b55`).
4. Clone the repo and run `rustup override set 1.87` to keep the workspace pinned.

## Build & Test Workflow
- `TEST_TRACE=on cargo test --test basic` — quick sanity check
- `cargo check --workspace` — fast validation across all crates.
- `cargo build --workspace --all-targets` — compile library + every actor binary.
- `cargo test --workspace` — full unit + integration suite; use `cargo test --test discovery` to focus on a single scenario.
- `cargo clippy --workspace --all-targets -- -D warnings` — lint gate.
- `cargo fmt --all` — enforce `rustfmt` config (`hard_tabs = true`, width 80).
- `mdbook serve docs --open` — preview docs changes locally.

## Actors & Development Loops
- Compile binaries with `just compile`, then launch the devnet via `just start`; stop with `just stop`.
- For manual iteration, `multipass shell mosaik-<actor>` (e.g., `mosaik-rpc`), run the binary (`./rpc -p 272a…d9b55`), and redeploy changes with the `just` cycle.
- Each actor crate owns its CLI flags; share new options in the corresponding crate README or `docs/book`.

## Contributing & Support
- Commits: concise, imperative subject referencing the component (e.g., “Progress on pubsub semantics”).
- PRs: include summary, linked issues/RFCs, checklist of `cargo build`, `cargo test`, `cargo clippy`, `cargo fmt`, and proof for user-visible changes (logs/screenshots).
- Documentation: update `docs/book` and `AGENTS.md` when protocol behavior or contributor flows change.
- Tests: add/extend integration coverage for behavioral updates and note any gaps or follow-ups in the PR body.
