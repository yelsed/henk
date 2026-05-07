# Linking projects

> **Status:** placeholder. Lands with **M4** (Docker mode) and **M6** (Host mode).

## Two modes

`henk link` (run inside a project directory) auto-picks one of two modes:

- **Docker mode** — the project has a `compose.yaml` / `docker-compose.yml`. henk writes `compose.override.yml` (or `henk.override.yml` if the canonical name is taken) with Traefik labels + the `henk-proxy` network, and writes `.henk.toml` as a marker.
- **Host mode** — the project runs natively (e.g. `nuxt dev`, `vite`) without a compose file. henk writes a Traefik file-provider entry to `~/.config/henk/dynamic/<slug>.yml` pointing at `http://host.docker.internal:<port>`. The project itself stays untouched apart from `.henk.toml`.

In both modes the project's existing daily commands (`npm run dev`, `sail up`, `docker compose up`, …) keep working. henk only handles routing.

## What henk writes

This document will list:

- The exact label set generated for Docker mode (single-host and multi-host).
- The schema of `.henk.toml`.
- The schema of file-provider YAML for Host mode.
- When henk falls back to `henk.override.yml` and what it asks the user to add to `.env`.
- When henk asks to append `APP_PORT=8080` (or analogous) to `.env`.
- What happens on `henk unlink`.

## Detection

`henk link` reads project files in priority order:

1. `compose.yaml` / `docker-compose.yml` services + published ports.
2. `.env` for `APP_URL` / `PUBLIC_URL` / `NUXT_BASE_URL` / `APP_BASE_URL` to choose the canonical service.
3. `package.json` `scripts.dev` / `scripts.start` for Host-mode port detection.
4. `vite.config.{js,ts,mjs}` or a `vite` dependency in `package.json` to offer the Vite sub-host (see [`vite-hmr.md`](vite-hmr.md)).

If the evidence is unambiguous, no prompt is shown. Otherwise the wizard asks just the missing pieces.
