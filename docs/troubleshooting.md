# Troubleshooting

> **Status:** placeholder. Filled in alongside each milestone as new failure modes are discovered.

`henk doctor` is the first thing to run when something feels off — it re-runs every detection probe and prints a pass/fail per check.

```sh
henk doctor              # diagnose
henk doctor --repair     # re-run any failed init steps surgically (M7+)
```

This document will cover, organised by symptom:

- "**`henk init` aborts with `host TCP :80 in use`**" — find and stop the offender; alternatively run `henk init --port-http <N> --port-https <M>` (planned).
- "**`https://app.test` shows a cert warning**" — check `mkcert -CAROOT`, re-run `henk init --regenerate-cert`.
- "**`https://app.test` resolves to nothing**" — check `/etc/resolver/<tld>` exists with henk's header, dnsmasq container is running, dnsmasq port matches the resolver file.
- "**Sail's `npm run dev` fails because port 80 is already taken**" — append `APP_PORT=8080` to the project's `.env` (henk normally prompts for this at `henk link` time; if you skipped it, add manually).
- "**Vite HMR doesn't connect**" — apply the `vite.config.js` snippet from [`vite-hmr.md`](vite-hmr.md), confirm `VITE_DEV_SERVER_URL` is set in `.env`.
- "**Container is up but `henk status` shows it as offline**" — check it joined the `henk-proxy` network (`docker network inspect henk-proxy`).
- "**`henk init` is partway done after a crash**" — the init steps are idempotent; just re-run `henk init` (or `henk doctor --repair`).

For anything not covered here, run `HENK_LOG=debug henk <command>` and include the stderr output when filing an issue.
