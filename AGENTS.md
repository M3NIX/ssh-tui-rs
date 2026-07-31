# AGENTS.md

## Repository overview

- `ssh-tui-rs` is a Rust terminal UI for browsing OpenSSH configuration and launching SSH sessions.
- The main crate metadata lives in `Cargo.toml`.
- Runtime code is under `src/`:
  - `main.rs` wires the CLI to the application.
  - `app.rs`, `ui.rs`, and `tree.rs` drive the TUI state and rendering.
  - `ssh_config.rs` parses OpenSSH configuration and resolves inherited host options.
  - `ssh.rs`, `embedded_session.rs`, and `reachability.rs` handle SSH launch, inline sessions, and host probes.
  - `search.rs` contains fuzzy search behavior.
- Regression coverage lives in `tests/`, with focused parser and large-config tests.
- Demo assets and recording helpers live in `demo/` and `assets/`.
- CI currently runs `cargo test --locked` from `.github/workflows/ci.yml`.

## Working guidelines

- Keep changes small and targeted.
- Preserve the project's read-only contract: the application must not modify SSH config files.
- Follow the existing Rust style and module boundaries instead of introducing new patterns without need.
- Update `README.md` when user-facing behavior, CLI options, or workflows change.
- Only touch demo assets or `demo/record.py` when the documented demo actually needs to change.

## Validation expectations

- Run the test suite with `cargo test --locked`.
- If a change affects packaging or installation flow, also verify the documented install path still makes sense.
- If a change affects the demo, regenerate it from the repository root with `python3 demo/record.py`.

## Commit message convention

Use Conventional Commit style prefixes:

- `feat:` new user-facing functionality
- `fix:` bug fixes
- `docs:` documentation-only changes
- `refactor:` internal code restructuring without behavior changes
- `test:` test additions or updates
- `ci:` CI or workflow changes
- `build:` dependency, release, or build-system changes
- `perf:` performance improvements
- `chore:` maintenance tasks that do not fit the categories above

Keep commit subjects short, imperative, and scoped to one change, for example:

- `docs: add repository agent guidance`
- `fix: preserve inherited SSH port resolution`
- `chore: refresh release metadata`
