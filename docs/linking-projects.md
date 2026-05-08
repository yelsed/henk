# Linking projects

## Two modes

`henk link` (run inside a project directory) auto-picks one of two modes:

- **Docker mode** — the project has a `compose.yaml` / `docker-compose.yml`. henk writes `compose.override.yml` (or `henk.override.yml` if the canonical name is taken) with Traefik labels + the `henk-proxy` network, and writes `.henk.toml` as a marker.
- **Host mode** — the project runs natively (e.g. `nuxt dev`, `vite`) without a compose file. henk writes a Traefik file-provider entry to `~/.config/henk/dynamic/<slug>.yml` pointing at `http://host.docker.internal:<port>`. The project itself stays untouched apart from `.henk.toml`.

In both modes the project's existing daily commands (`npm run dev`, `sail up`, `docker compose up`, …) keep working. henk only handles routing.

## What henk writes

### Docker mode

- `<project>/compose.override.yml` (or `<project>/henk.override.yml` when the canonical name is taken) — joins the project's web service to the `henk-proxy` network. Routing rules live in the global file-provider, **not** here, so the override stays small.
- `<project>/.henk.toml` — marker + slug + host list.
- `~/.config/henk/dynamic/<slug>.yml` — Traefik routers + services per host. Backend URL is `http://<service>:<port>` on the shared `henk-proxy` network.

If your service publishes `:80` or `:443` on the host (Sail's `${APP_PORT:-80}:80`), henk offers to append `APP_PORT=18080` (or another free high port) to your `.env` so Compose binds elsewhere. The append is consent-gated and idempotent — henk never edits existing lines.

When `compose.override.yml` is already yours, henk falls back to `henk.override.yml` and tells you to add `COMPOSE_FILE=compose.override.yml:henk.override.yml` to `.env` so Compose picks both up.

### Host mode

- `<project>/.henk.toml` — the only file henk writes inside the project.
- `~/.config/henk/dynamic/<slug>.yml` — file-provider entry pointing at `http://host.docker.internal:<port>`.

#### Bind your dev server to 0.0.0.0

Traefik runs in a container and reaches your dev server through `host.docker.internal`. Docker Desktop on macOS resolves that to a gateway address that **only sees ports bound on the IPv4 wildcard**. Frameworks like Nuxt, Vite, and Next default to `127.0.0.1` (or `[::1]` on newer Node), which the gateway can't reach — you'll get a `502 Bad Gateway` through `https://<slug>.<tld>`.

Pass the bind flag explicitly when you start the dev server:

```bash
# Nuxt
npm run dev -- --host 0.0.0.0 --port 3000

# Vite
npm run dev -- --host 0.0.0.0 --port 5173

# Next
npx next dev -H 0.0.0.0 -p 3000
```

Or set the bind address in your framework's config so plain `npm run dev` does the right thing. `henk link` prints this reminder at the end of every host-mode link.

## Detection

`henk link` reads project files in priority order:

1. `compose.yaml` / `docker-compose.yml` services + published ports.
2. `.env` for `APP_URL` / `PUBLIC_URL` / `NUXT_BASE_URL` / `APP_BASE_URL` to choose the canonical service.
3. `package.json` `scripts.dev` / `scripts.start` for Host-mode port detection.
4. `vite.config.{js,ts,mjs}` or a `vite` dependency in `package.json` to offer the Vite sub-host (see [`vite-hmr.md`](vite-hmr.md)).

If the evidence is unambiguous, no prompt is shown. Otherwise the wizard asks just the missing pieces.
