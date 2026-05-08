# henk

Laravel Valet-style local URLs (`https://myapp.test`) for any Docker container on macOS — trusted certs included, no `/etc/hosts` edits, no nginx config.

```sh
cd ~/projects/myapp
henk link
docker compose up
# open https://myapp.test
```

henk runs a small Traefik + dnsmasq stack in Docker. It owns `/etc/resolver/<tld>` so `*.test` resolves to it, uses `mkcert` to generate a wildcard cert your browsers already trust, and writes a `compose.override.yml` alongside your project. Your existing `docker compose up` workflow doesn't change.

Coexists with Laravel Valet and Herd — if `.test` is taken, henk falls back to `.henk` automatically.

---

## Install

**Prerequisites:** macOS (Apple Silicon or Intel), [Docker Desktop](https://www.docker.com/products/docker-desktop/) running, [Homebrew](https://brew.sh/) installed.

```sh
curl -fsSL https://github.com/fivespark/henk/releases/latest/download/henk-installer.sh | sh
```

Then run first-time setup:

```sh
henk init
```

`henk init` detects what's already on your machine, installs `mkcert`, `nss`, and `dnsmasq` via Homebrew (with your consent), writes `/etc/resolver/test`, generates a wildcard cert, and starts the global stack.

---

## Quickstart

```sh
henk init                  # one-time setup (2–3 min)
cd ~/projects/myapp
henk link                  # detect stack, write compose.override.yml
docker compose up          # your normal command, unchanged
open https://myapp.test    # done
```

---

## Recipes

### Laravel Sail + Vite (spatiebalk-style)

Sail already has a `compose.yaml`. henk writes a sibling `compose.override.yml` with Traefik labels, then offers a second host for Vite HMR.

```sh
cd ~/projects/spatiebalk
henk link                  # picks up APP_URL from .env, routes :80 → spatiebalk.test
henk link --add \
  --host vite.spatiebalk.test \
  --service laravel.test \
  --port 5173               # adds the Vite sub-host
```

Add the Vite config snippet henk prints to your `vite.config.js`:

```js
server: {
  host: '0.0.0.0',
  hmr: { host: 'vite.spatiebalk.test', protocol: 'wss', clientPort: 443 },
}
```

Set `VITE_PORT=15173` in `.env` if something else holds 5173, and add `VITE_DEV_SERVER_URL=https://vite.spatiebalk.test` so Laravel Vite Plugin finds it.

See [docs/vite-hmr.md](docs/vite-hmr.md) for the full label set and framework variations.

---

### Multi-service Docker Compose (hub-style: Directus + Mailhog)

```sh
cd ~/projects/hub
henk link                            # auto-detects web service → hub.test
henk link --add \
  --host mail.hub.test \
  --service mailhog \
  --port 8025                         # adds Mailhog under mail.hub.test
```

See [docs/multi-host.md](docs/multi-host.md) for the full `--add` flow and `.henk.toml` schema.

---

### Native dev server — no compose (sparkle-style: Nuxt)

For projects running directly on the host (not in Docker), henk uses a Traefik file-provider entry pointing at `host.docker.internal:<port>`.

```sh
cd ~/projects/sparkle
henk link                  # detects Nuxt, no compose.yaml → host mode
```

Set `devServer.host: '0.0.0.0'` (or pass `--host 0.0.0.0`) so the dev server binds on the IPv4 wildcard — Docker Desktop can only reach ports bound on `0.0.0.0`, not `127.0.0.1`.

```ts
// nuxt.config.ts
export default defineNuxtConfig({
  devServer: { host: '0.0.0.0', port: 3000 }
})
```

henk prints this reminder at the end of every host-mode link. See [docs/linking-projects.md](docs/linking-projects.md) for details.

---

## CLI reference

| Command | Description |
|---|---|
| `henk init` | First-run setup: detect prereqs, install tools, start global stack. |
| `henk init --dry-run` | Run all detection steps without making any system changes. |
| `henk init --tld <tld>` | Override the auto-picked TLD (default: `test`). |
| `henk link` | Register the project in the current directory. |
| `henk link --add` | Add another hostname to an already-linked project. |
| `henk link --host <h>` | Override the auto-detected hostname. |
| `henk link --service <s> --port <p>` | Override service + port (useful with `--add`). |
| `henk unlink` | Remove the current project from routing. |
| `henk unlink <host>` | Remove a single hostname from the current project. |
| `henk status` | Show stack health, linked projects, and cert state. |
| `henk up` | Start the global Traefik + dnsmasq stack. |
| `henk down` | Stop the global stack (and keep it stopped). |
| `henk doctor` | Run all detection + health checks. |
| `henk doctor --repair` | Re-run failed init steps surgically. |
| `henk update` | Self-update the henk binary from GitHub Releases. |
| `henk update --check` | Print whether a newer version is available. |
| `henk dashboard` | Live TUI: stack health, linked projects, cert state. |
| `henk uninstall` | Remove henk's files; keeps Homebrew packages. |
| `henk uninstall --deep` | Also uninstall Homebrew packages henk installed. |

`henk init --dry-run` is the recommended first command on a new machine — it produces a full detection report (Docker, Homebrew, mkcert, ports, coexistence tools) without touching anything.

---

## Preflight checks at link time

When you run `henk link`, henk surfaces any configuration issues it finds (wrong bind address, conflicting port, missing `.env` key) with paste-ready fix snippets, before writing any files. Nothing is written until the preflight passes.

---

## Architecture

```
              *.<tld>
                 │
       /etc/resolver/<tld>          macOS resolver → 127.0.0.1:35353
                 │
       ┌─────────┴────────────────┐
       │  henk global stack       │
       │  ┌────────┐ ┌──────────┐ │
       │  │traefik │ │ dnsmasq  │ │
       │  │:80 :443│ │:35353    │ │
       │  └────┬───┘ └──────────┘ │
       │       │  watches socket  │
       │       │  reads dynamic/  │
       └───────┼──────────────────┘
               │
   ┌───────────┴────────────────────┐
   │                                │
   ▼                                ▼
DOCKER MODE                    HOST MODE
compose.override.yml           file provider →
+ henk-proxy network           host.docker.internal:<port>
```

See [docs/architecture.md](docs/architecture.md) for the full picture.

---

## Documentation

- [Getting started](docs/getting-started.md)
- [Linking projects](docs/linking-projects.md)
- [Multi-host projects](docs/multi-host.md)
- [Vite HMR over HTTPS](docs/vite-hmr.md)
- [Lifecycle](docs/lifecycle.md)
- [Upgrading](docs/upgrading.md)
- [Coexistence with Valet/Herd/DDEV/Lando](docs/coexistence.md)
- [Customisation](docs/customization.md)
- [Troubleshooting](docs/troubleshooting.md)
- [Uninstall](docs/uninstall.md)
- [Architecture](docs/architecture.md)

---

## Development

```sh
cargo build                      # debug build
cargo build --release            # release build
cargo test                       # 84 tests across 9 milestones
cargo run -- init --dry-run      # run a subcommand
HENK_LOG=debug cargo run -- ...  # verbose tracing output
```

---

## License

[MIT](LICENSE)
