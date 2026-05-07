//! Minimal Compose-file reader.
//!
//! We only need:
//!   - the list of service names,
//!   - their image/build (for datastore filtering),
//!   - their published port mappings (for "which port should we route to?"),
//!
//! We don't need anchors, `!reset`, env-file resolution, or the full
//! schema. `serde_yaml_ng` happily skips unknown fields.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// A single project compose file.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ComposeFile {
    #[serde(default)]
    pub services: BTreeMap<String, ComposeService>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ComposeService {
    /// Set when the service uses a prebuilt image (e.g. `image: postgres:17`).
    #[serde(default)]
    pub image: Option<String>,
    /// Set when the service builds locally — we don't care about the
    /// build context, just the presence.
    #[serde(default)]
    pub build: Option<serde_yaml_ng::Value>,
    /// Raw port specs: short form (`"80:80"`, `"5173"`) and long form
    /// (`{ target: 80, published: 80 }`) both pass through here.
    #[serde(default)]
    pub ports: Vec<serde_yaml_ng::Value>,
}

impl ComposeService {
    /// Heuristic: returns true if the service looks like a datastore
    /// (postgres, redis, …) and should be excluded from "which one is
    /// the web service?" detection.
    pub fn looks_like_datastore(&self, name: &str) -> bool {
        let lower_name = name.to_ascii_lowercase();
        let lower_image = self
            .image
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        crate::consts::DATASTORE_PATTERNS
            .iter()
            .any(|pat| lower_name.contains(pat) || lower_image.contains(pat))
    }

    /// Extract `(host_port, container_port)` pairs from this service's
    /// `ports:` list. Handles both short-form strings and long-form maps.
    /// Skips entries we can't make sense of.
    pub fn published_ports(&self) -> Vec<PublishedPort> {
        let mut out = Vec::new();
        for raw in &self.ports {
            if let Some(p) = parse_port_value(raw) {
                out.push(p);
            }
        }
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublishedPort {
    /// Host-side port (the one curl/browser hits). May be 0 if compose
    /// only declares the container side.
    pub host: u16,
    /// Container-side port (the one inside the container, what Traefik
    /// wants to talk to).
    pub container: u16,
}

fn parse_port_value(value: &serde_yaml_ng::Value) -> Option<PublishedPort> {
    match value {
        serde_yaml_ng::Value::String(s) => parse_port_string(s),
        serde_yaml_ng::Value::Number(n) => {
            let n = n.as_u64()?;
            if n > u16::MAX as u64 {
                return None;
            }
            Some(PublishedPort {
                host: n as u16,
                container: n as u16,
            })
        }
        serde_yaml_ng::Value::Mapping(m) => {
            // long form: `{ target: 80, published: 80 }`
            let target = m
                .get(serde_yaml_ng::Value::String("target".into()))
                .and_then(|v| v.as_u64())?;
            let published = m
                .get(serde_yaml_ng::Value::String("published".into()))
                .and_then(|v| v.as_u64())
                .unwrap_or(target);
            if target > u16::MAX as u64 || published > u16::MAX as u64 {
                return None;
            }
            Some(PublishedPort {
                host: published as u16,
                container: target as u16,
            })
        }
        _ => None,
    }
}

/// Parse `"80:80"`, `"127.0.0.1:80:80"`, `"5173"`, `"${APP_PORT:-80}:80"`.
/// Skips entries we can't extract two integers from.
///
/// Compose port strings often embed `${VAR:-N}` defaults that contain a
/// literal `:` of their own — we must expand those before splitting on
/// `:`, otherwise the segment count goes haywire.
fn parse_port_string(s: &str) -> Option<PublishedPort> {
    // Expand `${VAR:-N}` -> `N`. Pure replace; no env lookup.
    let expanded = expand_shell_defaults(s);

    let segments: Vec<&str> = expanded.split(':').collect();
    let (host, container) = match segments.as_slice() {
        [single] => (single.trim(), single.trim()),
        [host, container] => (host.trim(), container.trim()),
        [_ip, host, container] => (host.trim(), container.trim()),
        _ => return None,
    };
    let host_port = host.parse::<u16>().ok()?;
    let container_port = container.parse::<u16>().ok()?;
    Some(PublishedPort {
        host: host_port,
        container: container_port,
    })
}

fn expand_shell_defaults(s: &str) -> String {
    // ${VAR:-DEFAULT}  →  DEFAULT
    // Anchored to the literal `:-` so we don't catch other `${...}` forms.
    use std::sync::LazyLock;
    static RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"\$\{[^}]*:-([^}]+)\}").expect("static regex")
    });
    RE.replace_all(s, "$1").into_owned()
}

/// Find the project's compose file, respecting Compose's discovery
/// order: `compose.yaml`, `compose.yml`, `docker-compose.yaml`,
/// `docker-compose.yml`.
pub fn find_compose_path(dir: &Path) -> Option<PathBuf> {
    for name in ["compose.yaml", "compose.yml", "docker-compose.yaml", "docker-compose.yml"] {
        let p = dir.join(name);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Parse a compose file from disk.
pub fn read(path: &Path) -> Result<ComposeFile> {
    let body = fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let cf: ComposeFile = serde_yaml_ng::from_str(&body)
        .with_context(|| format!("parsing {} as compose YAML", path.display()))?;
    Ok(cf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_spatiebalk_shaped_compose() {
        let yaml = r#"
services:
    laravel.test:
        image: sail-8.4/app
        ports:
            - "${APP_PORT:-80}:80"
            - "${VITE_PORT:-5173}:${VITE_PORT:-5173}"
            - "${REVERB_PORT:-8081}:${REVERB_PORT:-8081}"
        depends_on:
            - pgsql
    pgsql:
        image: postgres:17-alpine
        ports:
            - "5432:5432"
"#;
        let cf: ComposeFile = serde_yaml_ng::from_str(yaml).unwrap();
        assert!(cf.services.contains_key("laravel.test"));
        assert!(cf.services.contains_key("pgsql"));

        let app = &cf.services["laravel.test"];
        let ports = app.published_ports();
        assert_eq!(ports.len(), 3);
        assert_eq!(ports[0], PublishedPort { host: 80, container: 80 });
        assert_eq!(ports[1], PublishedPort { host: 5173, container: 5173 });

        // Datastore filter
        assert!(!app.looks_like_datastore("laravel.test"));
        assert!(cf.services["pgsql"].looks_like_datastore("pgsql"));
    }

    #[test]
    fn parses_long_form_ports() {
        let yaml = r#"
services:
  web:
    image: nginx
    ports:
      - target: 80
        published: 8080
"#;
        let cf: ComposeFile = serde_yaml_ng::from_str(yaml).unwrap();
        let ports = cf.services["web"].published_ports();
        assert_eq!(ports[0], PublishedPort { host: 8080, container: 80 });
    }

    #[test]
    fn parses_ip_prefixed_ports() {
        let yaml = r#"
services:
  web:
    image: nginx
    ports:
      - "127.0.0.1:8080:80"
"#;
        let cf: ComposeFile = serde_yaml_ng::from_str(yaml).unwrap();
        let ports = cf.services["web"].published_ports();
        assert_eq!(ports[0], PublishedPort { host: 8080, container: 80 });
    }

    #[test]
    fn datastore_pattern_filters_redis_and_postgres() {
        let yaml = r#"
services:
  redis:
    image: redis:7
  cache:
    image: redis:7
  postgres:
    image: postgres:16
  web:
    image: nginx
"#;
        let cf: ComposeFile = serde_yaml_ng::from_str(yaml).unwrap();
        assert!(cf.services["redis"].looks_like_datastore("redis"));
        assert!(cf.services["cache"].looks_like_datastore("cache")); // image triggers
        assert!(cf.services["postgres"].looks_like_datastore("postgres"));
        assert!(!cf.services["web"].looks_like_datastore("web"));
    }
}
