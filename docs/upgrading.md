# Upgrading

> **Status:** placeholder. Lands with **M7**.

```sh
henk update              # download + replace the binary
henk update --check      # report only, don't install
```

`henk` self-updates via [`axoupdater`](https://github.com/axodotdev/axoupdater) — it pulls the latest tag from GitHub Releases, verifies checksums, and replaces the running binary. The next henk command notices that the embedded `STACK_VERSION` is ahead of what's recorded in `state.json` and runs any required template / schema migrations.

Migrations are idempotent and additive. They never destroy data.

This document will cover:

- Soft "newer version available" notice cadence (only when ≥1 minor version stale, only on TTYs, never blocking).
- Disabling the update check (`update_check = false` in `~/.config/henk/config.toml`, `--no-update-check` flag, `HENK_NO_UPDATE_CHECK=1`).
- What network calls the update path makes (one HTTPS request to `api.github.com`).
- How to roll back to a previous version.
