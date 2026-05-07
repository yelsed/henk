# Architecture

This is the contributor-facing overview. User-facing docs are in the rest of `docs/`.

## High-level

```
              *.<tld> (default .test, fallback .henk)
                 │
       /etc/resolver/<tld>            macOS resolver: 127.0.0.1:35353
                 │
       ┌─────────┴────────────────────────┐
       │  henk global stack (Docker)      │
       │  ┌─────────┐  ┌────────────────┐ │
       │  │ traefik │  │   dnsmasq      │ │
       │  │:80, :443│  │127.0.0.1:35353 │ │
       │  └────┬────┘  └────────────────┘ │
       │       │  watches Docker socket   │
       │       │  + reads file-provider   │
       │       │     dynamic configs      │
       └───────┼──────────────────────────┘
               │
   ┌───────────┴────────────────────────────────┐
   │                                            │
   ▼                                            ▼
DOCKER MODE                                  HOST MODE
labels + henk-proxy network                  Traefik file provider →
(spatiebalk, hub, …)                         host.docker.internal:<port>
                                             (sparkle, …)
```

## Crate layout

```
src/
├── main.rs              # entrypoint
├── lib.rs               # module declarations (also used by integration tests)
├── consts.rs            # STACK_VERSION, default ports, file headers, web-port preference list
├── cli/                 # clap subcommands + dispatch
│   ├── mod.rs           # Cli struct + Command enum + dispatch()
│   ├── init.rs link.rs status.rs up.rs down.rs
│   ├── doctor.rs uninstall.rs update.rs unlink.rs
│   └── default.rs       # `henk` (no args) — smart context-aware status
├── detect/              # read-only probes (M1 — implemented)
│   ├── mod.rs           # DetectionReport + run_all()
│   ├── docker.rs brew.rs ports.rs resolver.rs
│   ├── coexistence.rs   # Valet/Herd/DDEV/Lando
│   └── tld.rs           # TLD decision logic (default / fallback / override)
├── stack/               # global Traefik+dnsmasq stack (M2/M3)
├── project/             # link/unlink, override-file generation (M4/M6)
├── manifest/            # state.json read/write + migrations (M7)
├── runner/              # tokio::process wrapper (used everywhere)
└── tui/                 # ratatui dashboard (M8) + inquire wizard (M5)

assets/                  # include_str!()'d templates rendered at runtime
├── traefik/             # global stack templates
├── dnsmasq/
└── project/             # per-project compose.override.yml.tmpl

tests/                   # assert_cmd integration + insta snapshots
                         # + non-destructive guarantee suite
docs/                    # user + contributor docs (this folder)
```

## Detection model (M1)

`detect::run_all(runner, tld_override)` returns a `DetectionReport` containing:

- A list of `DetectionItem { name, status, detail }` rows.
- A `TldChoice { value, reason }` recording which TLD will be used and why.

Each probe is read-only and asynchronous. Probes shell out via `runner::SystemRunner` (a thin `tokio::process::Command` wrapper). The whole report is rendered as a single coloured table by `DetectionReport::print()`.

Status semantics:

| Status | Icon | Meaning |
|---|---|---|
| `Ok` | ✓ green | Healthy / present / safe to proceed. |
| `Warn` | ! yellow | Worth flagging. Does not block. |
| `Block` | ✗ red | Hard collision. `henk init` aborts (M5+). |
| `Info` | i grey | Pure informational note. |

`DetectionReport::has_blockers()` is the boolean the wizard uses to decide whether to continue.

## Concurrency

We use `tokio` end-to-end, but most detection is fast (single shell-outs). Probes are awaited sequentially in `detect::run_all`; the cumulative time on a healthy machine is well under a second. We may parallelise later if needed.

## Non-destructive guarantees (planned for M4/M5/M7)

These are enforced by:

1. **Header tagging.** Every file henk writes carries `# managed by henk` (or its TOML equivalent). `unlink` / `uninstall` only delete files whose header matches.
2. **State manifest.** `~/.config/henk/state.json` records what henk created, including pre-existing-or-not flags for Homebrew packages. `uninstall --deep` only removes packages henk installed.
3. **Atomic writes.** Generated config files use temp-file-then-rename so a crash mid-write never leaves a half-written file in place.
4. **`compose.override.yml` collision check.** If the canonical filename already exists, henk falls back to `henk.override.yml` and prompts the user to add a `COMPOSE_FILE=` line to `.env`. We never silently overwrite.
5. **`.env` append-only.** Existing lines are never modified. New lines are added with consent.
6. **Resolver file ownership.** A pre-existing `/etc/resolver/<tld>` without our header forces a TLD change rather than overwriting.

A CI test suite asserts each rule with concrete fixtures.

## Coding conventions

- `anyhow::Result` at CLI boundaries; `thiserror` for library errors.
- One file = one logical unit. Modules are small.
- `pub` only what the test or sibling module needs.
- Logs go through `tracing`. The user-facing CLI prints directly to stdout; logs go to stderr and are filtered by `HENK_LOG`.
- Templates live in `assets/` and are embedded via `include_str!`. No filesystem path lookups for shipped templates.

## Building + testing

```sh
cargo check
cargo test
cargo run -- init --dry-run        # the M1 demo
cargo build --release              # produces target/release/henk
```

Integration tests use `assert_cmd` to invoke the binary and `insta` to snapshot rendered config files.
