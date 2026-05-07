# Coexistence with Valet, Herd, DDEV, Lando

> **Status:** placeholder. Lands with **M5/M7**.

`henk` is built to share a machine with other local-dev tools without breaking them. The core promises:

- `.test` is left alone if Laravel Valet or Laravel Herd is detected. henk falls back to `.henk` automatically.
- `/etc/resolver/<tld>` files henk did not write are never modified or removed.
- Ports 80 and 443 are never silently rebound. If something else is listening, `henk init` aborts with a clear error and a pointer at the offending PID.
- `henk-proxy` is the only Docker network henk creates; if a network with that name already exists and isn't ours, init aborts.

This document will cover:

- The exact detection signals for each tool.
- What "block" / "warn" / "info" means for each one.
- How to run `henk` alongside DDEV (different TLDs, possibly different ports).
- Recovery: the `henk doctor` view of coexistence findings + how to repair specific issues.
