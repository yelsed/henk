# Uninstalling henk

> **Status:** placeholder. Lands with **M7**.

Two reversal levels:

```sh
henk uninstall                  # remove only henk's own files
henk uninstall --deep           # also remove Homebrew packages henk installed
henk uninstall --keep-config    # stop the stack, keep ~/.config/henk for re-init
```

What `henk uninstall` (default) does:

- Stops and removes the global Traefik + dnsmasq containers.
- Removes the `henk-proxy` Docker network.
- Removes the wildcard cert files in `~/.config/henk/traefik/certs/`.
- Removes `/etc/resolver/<tld>` **only if** it carries the `# managed by henk` header (sudo prompt).
- Removes `~/.config/henk/` and `~/.local/share/henk/`.

What it explicitly **does not** do unless you pass `--deep`:

- `mkcert -uninstall` (other tools may rely on the root CA in your keychain).
- `brew uninstall mkcert nss` — even with `--deep`, only removes packages whose `state.json` entry shows `installed_by: henk`. Foreign-installed packages survive.

Per-project reversal (`henk unlink`, run inside a linked project) removes the routing entry, deletes the override file *only if it's ours*, and deletes `.henk.toml`. It does **not** revert `.env` changes (e.g. `APP_PORT=8080`) — you own those edits.

This document will cover:

- Full audit: what's left on the machine after `henk uninstall` vs `--deep`.
- How to inspect `~/.config/henk/state.json` to see what henk thinks it owns.
- Manual reversal if `henk` itself is broken.
