# henk

Local-dev URL routing for Docker on macOS — Valet/Herd ergonomics for any container.

```text
$ cd ~/yelsed/spatiebalk
$ henk link
$ npm run dev
$ open https://spatiebalk.test
```

`henk` runs a small global Traefik + dnsmasq stack in Docker, owns `/etc/resolver/<tld>` so `*.test` (or `*.henk`) resolves to it, and uses `mkcert` to generate a wildcard cert your browsers already trust. Each project gets registered with one command — `henk` writes a sibling `compose.override.yml` and never edits your `compose.yaml`.

It coexists with Laravel Valet/Herd, DDEV, and Lando. Where they own `.test`, `henk` falls back to `.henk` automatically.

## Status

Pre-release. M1 (CLI skeleton + detection) is complete — `henk init --dry-run` produces the full detection report. Subsequent milestones land per [`docs/architecture.md`](docs/architecture.md) and the implementation plan.

## Install

Not on a release tag yet. To run from source:

```sh
cargo build --release
./target/release/henk init --dry-run
```

When the first release ships:

```sh
curl -fsSL https://github.com/fivespark/henk/releases/latest/download/install.sh | sh
```

Single binary, macOS-only (darwin/aarch64 + darwin/x86_64). Uninstallable via `henk uninstall`.

## How it works

```
              *.<tld>
                 │
       /etc/resolver/<tld>            macOS resolver: 127.0.0.1:35353
                 │
       ┌─────────┴────────────────┐
       │  henk global stack       │
       │  ┌────────┐ ┌──────────┐ │
       │  │traefik │ │ dnsmasq  │ │
       │  │:80 :443│ │:35353    │ │
       │  └────┬───┘ └──────────┘ │
       │       │                  │
       │  watches Docker socket   │
       │  + reads dynamic configs │
       └───────┼──────────────────┘
               │
   ┌───────────┴────────────────────┐
   │                                │
   ▼                                ▼
DOCKER MODE                       HOST MODE
override.yml + henk-proxy net     Traefik file provider →
(spatiebalk, hub, …)              host.docker.internal:<port>
                                  (sparkle, …)
```

## Documentation

User docs live in [`docs/`](docs/):

- [Getting started](docs/getting-started.md)
- [Linking projects](docs/linking-projects.md)
- [Multi-host projects](docs/multi-host.md)
- [Lifecycle](docs/lifecycle.md)
- [Upgrading](docs/upgrading.md)
- [Coexistence with Valet/Herd/DDEV](docs/coexistence.md)
- [Vite HMR over HTTPS](docs/vite-hmr.md)
- [Customisation](docs/customization.md)
- [Troubleshooting](docs/troubleshooting.md)
- [Uninstall](docs/uninstall.md)
- [Architecture](docs/architecture.md) (for contributors)

## License

MIT.
