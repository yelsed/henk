# Getting started

> **Status:** M1 only. Full setup ships with M5; right now this guide covers what works today.

## Prerequisites

- macOS (Apple Silicon or Intel).
- [Docker Desktop](https://www.docker.com/products/docker-desktop/) — install + start it before running `henk`.
- [Homebrew](https://brew.sh/) — needed at `henk init` time so we can install `mkcert` and `nss` with your consent.

You do **not** need to install `mkcert`, `nss`, or `dnsmasq` yourself — `henk init` will offer to install them for you.

## Build from source (pre-release)

```sh
git clone https://github.com/yelsed/henk
cd henk
cargo build --release
sudo install target/release/henk /usr/local/bin/henk     # optional: put it on $PATH
```

Once a release tag exists:

```sh
curl -fsSL https://github.com/yelsed/henk/releases/latest/download/henk-installer.sh | sh
```

## What works today (M1)

```sh
henk --version
henk --help
henk                       # smart no-args status (currently a stub)
henk init --dry-run        # full detection report (no system writes)
```

`henk init --dry-run` walks every probe and prints a coloured table:

- Docker installed + running
- Homebrew, mkcert, nss
- Laravel Valet, Laravel Herd, DDEV, Lando coexistence
- `/etc/resolver/<tld>` ownership
- Ports 80, 443, and 35353
- The `henk-proxy` Docker network and any foreign Traefik containers
- The TLD henk would use (`.test` by default; `.henk` if Valet/Herd is detected)

If anything in the report is marked `✗` (red), `henk init` will refuse to proceed once full mode lands. Resolve those first.

## What's coming next

| Milestone | What ships |
|---|---|
| M2 | Traefik bring-up via `henk up` / `henk down`. |
| M3 | mkcert + dnsmasq-in-container + `/etc/resolver/<tld>` writer → real `https://*.test`. |
| M4 | `henk link` / `henk unlink` / `henk status` for Docker-mode projects. |
| M5 | Full TUI wizard for `henk init`. |
| M6 | Host-mode projects (no compose; Traefik file provider → `host.docker.internal`). |
| M7 | `doctor`, `update`, `uninstall` (tiered), state-manifest migrations. |
| M8 | Live `henk dashboard` TUI. |

See [`docs/architecture.md`](architecture.md) for the bigger picture.
