# Troubleshooting

> **Status:** placeholder. Filled in alongside each milestone as new failure modes are discovered.

`henk doctor` is the first thing to run when something feels off — it re-runs every detection probe and prints a pass/fail per check.

```sh
henk doctor              # diagnose
henk doctor --repair     # re-run any failed init steps surgically (M7+)
```

## What henk's error pages tell you

When a request reaches henk but can't reach a healthy app, henk answers with a page that names the cause rather than a bare status code. Each page comes in two formats: browsers get HTML, and anything that doesn't ask for HTML — `curl`, `fetch`, a script, a coding agent — gets the same content as plain text it can actually read.

| You get | It means |
| --- | --- |
| **"Dev server isn't answering"** (503) | Routing and the certificate are fine; nothing usable came back from the address you linked. Almost always: the dev server isn't running, it's bound to `127.0.0.1` instead of `0.0.0.0`, or it rejects the `.test` hostname. `henk doctor` probes the port and tells you which. |
| **"Your app answered — with an error"** (the app's own 5xx) | Not a henk problem. The request reached your app and your app returned a server error — the stack trace is in your dev server's terminal. |
| **"Nothing is linked to this hostname"** (404) | The stack is up but no project is registered under that name. `henk status` lists what is linked; `henk link` registers a new one. |

Two failures happen *before* any of this and so can't be paged at all: an untrusted or expired certificate (the TLS handshake fails first — you get the browser's interstitial, or `curl: (60)`), and the stack being down (nothing is listening, so you get a connection refused). Both are what `henk doctor` is for.

**Caveat on the "nothing is linked" page over https:** the wildcard certificate covers `*.test`, but macOS and curl refuse a wildcard directly under a public suffix, so henk also lists every linked host as an explicit certificate name. A hostname that was never linked isn't on that list — so a genuine typo trips the certificate warning *before* the page can render. Over http you'll see the page; over https, expect `curl: (60)` / a browser interstitial for a hostname henk has never heard of.

This document will cover, organised by symptom:

- "**`henk init` aborts with `host TCP :80 in use`**" — find and stop the offender; alternatively run `henk init --port-http <N> --port-https <M>` (planned).
- "**`curl http://app.test` returns `404 page not found`**" — you're on an old stack. henk's routers used to be https-only, so plain http matched nothing; each linked host now also gets an http router that redirects to https. Run `henk up` to re-render and restart the proxy.
- "**`https://app.test` shows a cert warning**" — check `mkcert -CAROOT`, re-run `henk init --regenerate-cert`.
- "**`https://app.test` resolves to nothing**" — check `/etc/resolver/<tld>` exists with henk's header, dnsmasq container is running, dnsmasq port matches the resolver file.
- "**Sail's `npm run dev` fails because port 80 is already taken**" — append `APP_PORT=8080` to the project's `.env` (henk normally prompts for this at `henk link` time; if you skipped it, add manually).
- "**Vite HMR doesn't connect**" — apply the `vite.config.js` snippet from [`vite-hmr.md`](vite-hmr.md), confirm `VITE_DEV_SERVER_URL` is set in `.env`.
- "**Container is up but `henk status` shows it as offline**" — check it joined the `henk-proxy` network (`docker network inspect henk-proxy`).
- "**`henk init` is partway done after a crash**" — the init steps are idempotent; just re-run `henk init` (or `henk doctor --repair`).

For anything not covered here, run `HENK_LOG=debug henk <command>` and include the stderr output when filing an issue.
