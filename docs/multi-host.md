# Multi-host projects

> **Status:** placeholder. Lands with **M4**.

A project can have several hostnames. The `.henk.toml` schema carries a list of `[[hosts]]` blocks from day one — adding a second URL is `henk link --add` (you'll be prompted for host, service, and port).

Typical use cases:

- `app.test` (frontend) + `api.app.test` (backend service in the same compose).
- `hub.test` (Directus) + `mail.hub.test` (Mailhog) — see the `hub` example in the project plan.
- `app.test` (main) + `vite.app.test` (Vite HMR sub-host — auto-offered, see [`vite-hmr.md`](vite-hmr.md)).

This document will cover:

- `henk link --add` interactive flow.
- Multiple host entries in `.henk.toml`.
- How the generated `compose.override.yml` emits one Traefik router per host.
- Removing a single host with `henk unlink <host>` vs the whole project with `henk unlink --all`.
