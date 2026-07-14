# Lifecycle

> **Status:** placeholder. Lands with **M2/M3**.

The global Traefik + dnsmasq stack runs with `restart: unless-stopped`, so it auto-resumes whenever Docker Desktop starts. You shouldn't have to think about it after `henk init`.

`henk up` is idempotent: it checks `docker info` first, refuses to launch Docker for you, and is a no-op if the stack is already up. `henk down` stops the stack *and keeps it stopped* (`unless-stopped` honours explicit stops).

`henk up` is also the migration path. Every run re-renders the stack templates from the embedded ones, upgrades each linked project's routing to the current shape, and records the applied `STACK_VERSION` in `~/.config/henk/state.json`. Two of the generated files — `traefik.yml` and the error pages' `nginx.conf` — are only read when the containers boot (Traefik watches the *dynamic* directory, but not its own static config), so when their contents change `henk up` restarts the proxy; otherwise it leaves it running. See [upgrading.md](upgrading.md).

This document will cover:

- The exact compose file generated for the global stack.
- Cert renewal (mkcert wildcard certs are long-lived; `henk init --regenerate-cert` to force).
- What happens when Docker Desktop isn't running.
