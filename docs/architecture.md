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
├── errorpages/          # shell.html + one body per failure, in html and txt
├── dnsmasq/
└── project/             # per-project compose.override.yml.tmpl

tests/                   # assert_cmd integration + insta snapshots
                         # + non-destructive guarantee suite
docs/                    # user + contributor docs (this folder)
```

## Routing and error pages

Every linked host gets **two** routers, written by `project::file_provider::render`:

- `<name>` on `websecure` — the real one. Its service is a Traefik `failover`: the health-checked backend while it's up, `henk-error-pages` when it isn't.
- `<name>-http` on `web` — the same host, carrying only the `henk-https-redirect` middleware.

The redirect is deliberately **per-host** rather than a `redirections` block on the `web` entrypoint. An entrypoint-wide redirect also catches hostnames nobody linked and bounces them to https — where the certificate can't cover them (see below), so a typo would die in the TLS handshake instead of reaching a page that explains itself. Anything writing a router must also skip these twins when reading them back: they name the same host and service, so `detect::backend::parse_linked_hosts` filters them out or every linked host is listed and probed twice.

Unmatched hostnames fall through to `henk-catchall` (priority 1, the floor), which `replacePath`s to `/unlinked`.

**The pages themselves** live in `assets/errorpages/` and are served by an `nginx:alpine` container. `nginx.conf` is the only place that can see both the request path and the client's `Accept` header, so it decides two things at once:

- **Which page** — from the path. The `henk-errors` middleware fetches `/{status}.html`, so `502`/`503`/`504` (Traefik couldn't reach the backend, or it timed out) get the *down* page, while a `500` (an app that answered, with an error of its own) gets the *app-error* page. `/unlinked` gets the *unlinked* page. Everything else — the failover forwarding a visitor's original path — is the down page.
- **Which format** — from `Accept`. Browsers ask for `text/html` and get the styled page; `curl`, `fetch`, scripts and coding agents send `*/*` and get the same content as plain text they can actually read. `Vary: Accept` is set.

`error_page` has no `=` before its target, so nginx preserves the original status — a `200` here would tell a browser (and any script) that a dead app is fine.

**What can't be paged:** an untrusted or expired certificate, and the stack being down. Both fail before any HTTP handler exists. This also bounds the unlinked page: the wildcard cert covers `*.<tld>`, but macOS and curl reject a wildcard directly under a public suffix, so `stack::certs` names every linked host explicitly as a SAN. A never-linked host isn't on that list, so over https it fails the handshake — the unlinked page is reachable over http.

## Stack migration

`STACK_VERSION` (`consts.rs`) is the shape of the generated config, versioned independently of the binary. `stack::templates::render_all` returns whether a file the containers only read at *boot* changed (`traefik.yml`, `nginx.conf` — Traefik watches the dynamic directory but not its own static config); `stack::lifecycle::up` restarts the proxy when it did, migrates each project entry via `file_provider::migrate_legacy_entries`, and records the applied version in `state.json`.

That makes `henk up` the single migration path — `henk update` re-invokes the newly installed binary as `henk up`, and `henk status` / `henk doctor` nudge when the binary was replaced some other way. Project entries are rebuilt from the routing files themselves (`manifest_from_entry`), because `.henk.toml` lives inside each project and henk keeps no global index of where those are.

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
