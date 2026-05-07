# Customisation

> **Status:** placeholder. Extension hooks land in **v1.1**.

The Traefik / dnsmasq config files henk generates carry a header:

```yaml
# managed by henk — hand-edits will be overwritten on `henk update`.
# See docs/customization.md for safe extension points.
```

If you hand-edit them, the next `henk update` (or any command that triggers a `STACK_VERSION` migration) will overwrite your changes.

`henk customize` is planned as a v1.1 command that lets you drop user-owned snippets into well-defined extension points (e.g. extra Traefik middlewares, additional file-provider entries) without forking the templates.

This document will cover:

- The list of extension points and the YAML schemas for each.
- Where user customisations live on disk (separate from generated files).
- How `henk doctor` validates that user customisations don't conflict with the generated config.
