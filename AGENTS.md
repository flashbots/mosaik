# Repository Guidelines

## Project Structure & Module Organization
The workspace root hosts the shared crate in `src/`, with integration scenarios in `tests/` (e.g., `tests/pubsub.rs`). Specialized actors live under `actors/` as individual crates (`actors/builder`, `actors/rpc`, etc.) and compile as binaries that orchestrate the core library. Long-form documentation is maintained as an mdBook in `docs/book`; update it whenever protocol behavior changes to keep engineering and research aligned.

## Build, Test, and Development Commands
Use `cargo check --workspace` for a fast smoke test during iteration, and `cargo build --workspace --all-targets` before pushing to ensure every crate and actor binary compiles. `cargo test --workspace` runs unit plus integration tests, while targeted suites such as `cargo test --test discovery` focus on a single scenario. Run `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all` prior to submitting changes. For docs, render the book locally with `mdbook serve docs --open` so reviewers can validate diagrams and walkthroughs.

## Coding Style & Naming Conventions
Formatting is enforced via `rustfmt` with repository-specific rules (`hard_tabs = true`, width 80, reordered imports). Keep modules and functions in `snake_case`, exported types in `UpperCamelCase`, and constants in `SCREAMING_SNAKE_CASE`. Prefer explicit module boundaries over glob exports, and favor `tracing` spans for long-running async tasks. When touching async code, use `tokio::spawn` judiciously and bubble errors with `anyhow::Result`.

## Testing Guidelines
Integration tests in `tests/` should declare descriptive file names that mirror the subsystem (`pubsub`, `discovery`). Within crates, colocate `#[cfg(test)] mod tests` at the bottom of each file. Tests that exercise async paths should use `#[tokio::test(flavor = "multi_thread")]`. Keep fixtures deterministic and document bespoke data in comments. Aim to extend coverage whenever new behavior is added, and communicate any gaps in the PR body.

## Commit & Pull Request Guidelines
Recent history shows concise, imperative commit titles (e.g., “Progress on pubsub semantics”). Follow that pattern, reference the component touched, and describe the “why” in the body if needed. PRs must include: a short summary, linked issues or RFCs, a checklist of commands run (build, tests, clippy, fmt), and screenshots or logs for user-visible changes. Tag domain owners for cross-actor changes and call out any follow-up work or outstanding risks.
