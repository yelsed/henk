# Lifecycle

> **Status:** placeholder. Lands with **M2/M3**.

The global Traefik + dnsmasq stack runs with `restart: unless-stopped`, so it auto-resumes whenever Docker Desktop starts. You shouldn't have to think about it after `henk init`.

`henk up` is idempotent: it checks `docker info` first, refuses to launch Docker for you, and is a no-op if the stack is already up. `henk down` stops the stack *and keeps it stopped* (`unless-stopped` honours explicit stops).

This document will cover:

- The exact compose file generated for the global stack.
- Drift handling: when a `henk update` brings new templates, `henk` notices the embedded `STACK_VERSION` is newer than what's recorded in `~/.config/henk/state.json`, regenerates the templates, and re-applies via `docker compose up -d`.
- Cert renewal (mkcert wildcard certs are long-lived; `henk init --regenerate-cert` to force).
- What happens when Docker Desktop isn't running.
