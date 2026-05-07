//! Compile-time constants used throughout henk.

/// Bumped whenever the embedded Traefik / dnsmasq compose templates or schema
/// change in a way that requires re-rendering on the user's machine.
/// `henk` checks this against `state.json::stack_version` and migrates if newer.
pub const STACK_VERSION: u32 = 1;

/// Bumped on `state.json` schema changes.
pub const STATE_SCHEMA_VERSION: u32 = 1;

/// Bumped on `.henk.toml` schema changes.
pub const PROJECT_MANIFEST_VERSION: u32 = 1;

/// Default top-level domain used when no Valet/Herd is detected.
pub const DEFAULT_TLD: &str = "test";

/// Fallback TLD used when Valet/Herd own the `.test` resolver.
pub const FALLBACK_TLD: &str = "henk";

/// Port the in-stack dnsmasq listens on (binds 127.0.0.1 only).
/// Picked to be obscure and unlikely to collide with anything else.
pub const DNSMASQ_PORT: u16 = 35353;

/// Standard HTTP / HTTPS ports the global Traefik binds on the host.
pub const HTTP_PORT: u16 = 80;
pub const HTTPS_PORT: u16 = 443;

/// Shared Docker network name. Each linked Docker-mode project joins it.
pub const PROXY_NETWORK: &str = "henk-proxy";

/// Header inserted at the top of every file henk authors. Used by uninstall
/// and unlink to verify ownership before deleting.
pub const HENK_FILE_HEADER: &str = "# managed by henk — see https://github.com/fivespark/henk";

/// Equivalent header for TOML files (which need a `#` comment line too, just
/// kept separate for clarity / future divergence).
pub const HENK_TOML_HEADER: &str = "# managed by henk — see https://github.com/fivespark/henk";

/// Web-ish ports preferred when multiple are exposed by a service.
/// Lower index = stronger preference.
pub const WEB_PORTS: &[u16] = &[80, 443, 8080, 8000, 8055, 3000, 5173, 4321, 4200, 8025];

/// Image / service-name patterns we treat as datastores and exclude from
/// "which is the web service?" detection.
pub const DATASTORE_PATTERNS: &[&str] = &[
    "postgres",
    "pgsql",
    "mysql",
    "mariadb",
    "redis",
    "valkey",
    "memcached",
    "mongo",
    "elastic",
    "rabbitmq",
    "kafka",
    "minio",
    "meilisearch",
    "clickhouse",
];

/// Reserved TLDs we refuse to use even if the user asks.
pub const RESERVED_TLDS: &[&str] = &["local", "localhost", "example", "invalid"];
