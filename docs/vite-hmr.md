# Vite HMR over HTTPS

> **Status:** placeholder. Lands with **M4**.

When you load `https://app.test`, the page is HTTPS. Vite's HMR WebSocket has to use `wss://` and target the same origin (or a sub-origin), or browsers will block it for mixed-content reasons.

`henk link` detects Vite (via `package.json` or `vite.config.{js,ts,mjs}`) and offers to add a second route, e.g. `vite.app.test → port 5173` with WebSocket-aware Traefik labels. henk **never** auto-edits your `vite.config.*` — there are too many flavours to safely transform — but it prints the snippet you should paste:

```js
// vite.config.js
export default defineConfig({
  // ...
  server: {
    host: '0.0.0.0',
    hmr: { host: 'vite.app.test', protocol: 'wss', clientPort: 443 },
    cors: true,
  },
})
```

```env
# .env
VITE_DEV_SERVER_URL=https://vite.app.test
```

This document will cover:

- The full label set henk emits for the Vite sub-host (router rule, entrypoint, TLS, service, target port).
- How Laravel Vite Plugin reads `VITE_DEV_SERVER_URL` and how to verify it's pulling from `https://vite.app.test`.
- Adapting for Nuxt + other Vite-based dev servers.
- What to do if you decline the sub-host (the app still loads — only HMR is degraded).
