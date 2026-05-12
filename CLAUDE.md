# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What `henk` is

A macOS CLI (Rust, edition 2024) that gives Docker containers Laravel-Valet-style trusted HTTPS URLs (`https://myapp.test`) with no `/etc/hosts` edits and no hand-written nginx. It manages a global Traefik + dnsmasq stack (running in Docker), wires up macOS DNS via `/etc/resolver/<tld>`, and mints trusted wildcard certs via `mkcert`. Coexists with Valet/Herd/DDEV/Lando — when `.test` is already owned it falls back to `.henk`.

## Commands

```sh
cargo build                       # debug build
cargo build --release             # optimized; produces target/release/henk
cargo check --all-features        # fast typecheck (matches CI "check" job)
cargo test                        # all tests (unit + tests/cli.rs integration)
cargo clippy -- -D warnings       # lint (CI fails on any warning)
cargo fmt --check                 # format check (CI); `cargo fmt` to fix

# run the CLI from source
cargo run -- init --dry-run
HENK_LOG=debug cargo run -- status   # HENK_LOG controls tracing (default: warn), logs → stderr

# single test
cargo test roundtrip_empty_state                      # by test name, anywhere
cargo test project::compose::tests                    # a module's tests
cargo test --lib                                      # unit tests only
cargo test --test cli                                 # integration tests only (tests/cli.rs)
cargo test some_test -- --nocapture --test-threads=1  # with output, serialized
```

CI (`.github/workflows/ci.yml`) runs `check`, `test`, `clippy -D warnings`, `fmt --check` on `macos-latest`. Release (`.github/workflows/release.yml`, config in `dist-workspace.toml`) is `cargo-dist`-driven, triggered by a `v[0-9]+.[0-9]+.[0-9]+` tag; builds `aarch64-apple-darwin` + `x86_64-apple-darwin`, ships a shell installer, no crates.io publish. Self-update at runtime via `henk update` (uses `axoupdater`).

## Architecture

`src/main.rs` = thin entrypoint (init tracing → `cli::dispatch`). `src/lib.rs` declares the public modules (also consumed by `tests/cli.rs`). Everything is `tokio` async; shelling out goes through `runner::SystemRunner` (a `tokio::process::Command` wrapper) so it's swappable in tests.

Module map (each subsystem maps to a milestone — see `docs/architecture.md` for the full version):

- `cli/` — `clap` `Cli`/`Command` + `dispatch()`. Subcommands: `init`, `link`, `unlink`, `status`, `up`, `down`, `doctor`, `update`, `uninstall`, `dashboard`. `cli/default.rs` is the no-args smart status.
- `detect/` — read-only async probes (Docker, Homebrew/mkcert/dnsmasq, port collisions on :80/:443/:35353, `/etc/resolver/<tld>` ownership, Valet/Herd/DDEV/Lando, TLD decision). `detect::run_all(runner, tld_override) -> DetectionReport`; `report.has_blockers()` gates the init wizard. Status levels: `Ok`/`Warn`/`Block`/`Info`.
- `stack/` — the global Traefik + dnsmasq stack: lifecycle (`up`/`down`, idempotent, version-drift detection vs `STACK_VERSION`), template rendering, `mkcert` CA + wildcard cert, `/etc/resolver/<tld>` write, dnsmasq via Homebrew + launchd dropin, XDG paths.
- `project/` — per-project `link`/`unlink`. Detects the web service + port from `docker-compose.yaml` (`compose.rs` is a deliberately minimal YAML parser: services, image/build, ports incl. `${VAR:-default}` expansion, networks in list or map form). Two output modes: **Docker mode** writes `compose.override.yml` (falls back to `henk.override.yml` on collision) adding Traefik labels + the `henk-proxy` network; **Host mode** writes a Traefik file-provider YAML routing to `host.docker.internal:<port>` for native dev servers. `preflight.rs` does static analysis *before* writing anything (bad bind address, port collisions, missing `.env` keys, Vite HMR issues) and emits fix snippets. `env_file.rs` appends to `.env` (append-only, never edits existing lines). `manifest.rs` writes per-project `.henk.toml`.
- `manifest/` — `~/.config/henk/state.json`: append-only audit trail of every step, plus install attribution (which Homebrew packages henk installed vs. pre-existing). Drives `doctor` and `uninstall --deep`.
- `config/` — `~/.config/henk/config.toml`: user settings (TLD, ports, update-check toggle), schema-versioned.
- `tui/` — `inquire` interactive init wizard (M5) + `ratatui` live `dashboard` (M8).
- `consts.rs` — compile-time constants: `STACK_VERSION`, `DEFAULT_TLD`="test"/`FALLBACK_TLD`="henk", `DNSMASQ_PORT`=35353, `DASHBOARD_PORT`=19080, `PROXY_NETWORK`="henk-proxy", `HENK_FILE_HEADER`, `WEB_PORTS` preference list, `DATASTORE_PATTERNS`, `RESERVED_TLDS`.
- `assets/` — Traefik / dnsmasq / per-project compose templates, embedded via `include_str!` (no runtime filesystem lookup for shipped templates).

## Conventions & invariants

- `anyhow::Result` at CLI boundaries; `thiserror` for library error types.
- User-facing output → stdout (often colorized via `owo-colors`); logs → stderr via `tracing`, filtered by `HENK_LOG`.
- One file = one logical unit; keep modules small; `pub` only what siblings/tests need.
- Non-destructive by design — preserve these when touching `stack/` or `project/`:
  1. Every file henk writes starts with `HENK_FILE_HEADER` ("# managed by henk …"); `unlink`/`uninstall` only delete files whose header matches.
  2. `state.json` records what henk created (incl. pre-existing-or-not flags); `uninstall --deep` only removes packages henk installed.
  3. Generated configs are written temp-then-rename (atomic).
  4. Never silently overwrite `compose.override.yml` — fall back to `henk.override.yml`.
  5. `.env` is append-only.
  6. A pre-existing unowned `/etc/resolver/<tld>` forces a TLD change rather than an overwrite.
- Integration tests use `assert_cmd` to drive the binary; `insta` snapshots rendered config files (`cargo insta review` to update). Tests for the non-destructive guarantees live alongside.

## Further docs

`docs/architecture.md` (contributor overview), `docs/` (user docs: getting-started, linking-projects, multi-host, vite-hmr, lifecycle, upgrading, coexistence, customization, troubleshooting, uninstall), `README.md` (install + recipes).
