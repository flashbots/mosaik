# Repository Guidelines

## Project Structure & Module Organization
The workspace root holds the shared Rust crate in `src/`, with module-level tests colocated. Integration scenarios live under `tests/` (e.g., `tests/pubsub.rs`) and mirror subsystem names so failures point directly to the affected flow. Actor binaries reside in `actors/` (`actors/builder`, `actors/rpc`, etc.); each compiles as an orchestrator around the shared crate. Long-form documentation is authored through the mdBook in `docs/book`, so update the relevant chapter whenever protocol semantics or traces change.

## Build, Test, and Development Commands
Use `cargo check --workspace` for a fast validation of the entire tree before pushing intermediate work. Run `cargo build --workspace --all-targets` to ensure every crate and actor binary compiles together. Execute `cargo test --workspace` for the full suite, and prefer targeted runs like `cargo test --test discovery` when iterating on a single scenario. Lint with `cargo clippy --workspace --all-targets -- -D warnings`, and format with `cargo fmt --all` before committing. Render docs locally using `mdbook serve docs --open` to confirm diagrams and walkthroughs.

## Coding Style & Naming Conventions
Formatting is enforced via `rustfmt` with repo defaults (`hard_tabs = true`, width 80, reordered imports). Keep modules and functions in `snake_case`, exported structs/enums in `UpperCamelCase`, and constants in `SCREAMING_SNAKE_CASE`. Prefer explicit module exports over globs, and wrap long-running async work in `tracing` spans. When adding async tasks, route errors through `anyhow::Result` to preserve context.

## Testing Guidelines
Rust unit tests live near the code behind `#[cfg(test)] mod tests` blocks, while integration tests sit in `tests/`. Async paths should use `#[tokio::test(flavor = "multi_thread")]` to match production scheduling. Keep fixtures deterministic and document bespoke inputs inline. Extend or add targeted integration coverage whenever behavior changes, and note any remaining gaps in the PR body.

## Commit & Pull Request Guidelines
Follow the existing concise, imperative commit style (e.g., “Progress on pubsub semantics”) and mention the component touched. PRs must include: a short summary, linked issues or RFCs, a checklist of commands run (`cargo build`, `cargo test`, `cargo clippy`, `cargo fmt`), and screenshots or logs for user-visible changes. Tag domain owners for cross-actor edits and document follow-up work or known risks so reviewers can plan handoffs.
