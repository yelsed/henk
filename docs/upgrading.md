# Upgrading

```sh
henk update              # download + replace the binary, then upgrade the stack
henk update --check      # report only, don't install
```

## What `henk update` does

1. **Replaces the binary.** henk self-updates via [`axoupdater`](https://github.com/axodotdev/axoupdater) — it pulls the latest tag from GitHub Releases, verifies checksums, and replaces the running binary.
2. **Upgrades the stack to match.** A new binary ships new Traefik / nginx templates, but the containers keep running the config they booted with — an updated henk in front of a stale proxy still routes by the old rules. So `henk update` re-invokes the freshly installed binary as `henk up`, which re-renders the templates, migrates each linked project's routing, and restarts the proxy if its boot config changed.

Step 2 has to be done by the *new* binary: the process doing the updating is the old one, and it can only render the templates compiled into it.

## Two versions, not one

- **The binary version** (`henk --version`) — what you have installed.
- **`STACK_VERSION`** — the shape of the generated Traefik / nginx config. Bumped whenever the templates change in a way that needs re-rendering on your machine. The version last applied is recorded in `~/.config/henk/state.json`.

They move independently: a binary upgrade that doesn't touch the templates leaves `STACK_VERSION` alone.

## If the binary was replaced some other way

Homebrew, the installer script, `cargo install`, or a `git pull && cargo build` all swap the binary without going through `henk update` — so nothing re-renders the stack. henk notices on its own:

- `henk status` prints a one-line nudge (`The running stack is v3 but this henk ships v4`).
- `henk doctor` reports it as drift.

In both cases the fix is `henk up`, which re-renders, migrates, restarts if needed, and records the new version. It's idempotent: when nothing has changed it does nothing and restarts nothing.

Migrations are additive and never destroy data. Project entries are rebuilt from the routing files themselves, so henk doesn't need to find your project directories to upgrade them.

## Not yet covered

- Soft "newer version available" notice cadence (only when ≥1 minor version stale, only on TTYs, never blocking).
- Disabling the update check (`update_check = false` in `~/.config/henk/config.toml`, `--no-update-check` flag, `HENK_NO_UPDATE_CHECK=1`).
- How to roll back to a previous version.
