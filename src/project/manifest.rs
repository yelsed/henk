//! `.henk.toml` schema — the per-project marker file.
//!
//! Carries the host list, the chosen mode (Docker vs Host), and enough
//! context to re-render the compose override / file-provider entry on
//! `henk link --add` or recover after `henk doctor --repair`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::consts::PROJECT_MANIFEST_VERSION;

/// `<project>/.henk.toml`. Not the same as the global `state.json`.
pub const FILENAME: &str = ".henk.toml";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ProjectManifest {
    pub schema_version: u32,
    /// Short slug used in router/service names. Derived from the project
    /// directory name unless explicitly overridden.
    pub slug: String,
    /// `"docker"` or `"host"`.
    pub mode: ProjectMode,
    /// One or more hosts the project answers on. The first one is the
    /// canonical / default URL.
    pub hosts: Vec<HostEntry>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProjectMode {
    Docker,
    Host,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct HostEntry {
    /// Full hostname incl. TLD, e.g. `spatiebalk.test`.
    pub host: String,
    /// Container service name (Docker mode) — must match a key under
    /// `services:` in the project's compose file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    /// Container port the service listens on (Docker mode).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Backend URL the proxy forwards to (Host mode), e.g.
    /// `http://host.docker.internal:3000`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Free-form tags. Used today for `["vite"]` to mark the Vite HMR
    /// sub-host so the wizard knows not to offer it again on relink.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<String>,
}

impl ProjectManifest {
    pub fn new(slug: impl Into<String>, mode: ProjectMode) -> Self {
        Self {
            schema_version: PROJECT_MANIFEST_VERSION,
            slug: slug.into(),
            mode,
            hosts: Vec::new(),
        }
    }

    /// Path to the manifest inside `dir`.
    pub fn path_in(dir: &Path) -> PathBuf {
        dir.join(FILENAME)
    }

    /// Load the manifest from a project directory. Returns `None` if the
    /// file doesn't exist (project hasn't been linked yet).
    pub fn load(dir: &Path) -> Result<Option<Self>> {
        let p = Self::path_in(dir);
        if !p.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(&p)
            .with_context(|| format!("reading {}", p.display()))?;
        let parsed: Self = toml::from_str(&raw)
            .with_context(|| format!("parsing {} as .henk.toml", p.display()))?;
        Ok(Some(parsed))
    }

    /// Write the manifest atomically with our header.
    pub fn save(&self, dir: &Path) -> Result<()> {
        let p = Self::path_in(dir);
        let body = toml::to_string_pretty(self).context("serialising manifest")?;
        let header = format!(
            "# managed by henk — see https://github.com/fivespark/henk\n\
             # this file marks the project as linked. Safe to commit or .gitignore.\n",
        );
        let full = format!("{header}\n{body}");
        let tmp = p.with_extension("tmp");
        fs::write(&tmp, full).with_context(|| format!("writing {}", tmp.display()))?;
        fs::rename(&tmp, &p)
            .with_context(|| format!("renaming {} -> {}", tmp.display(), p.display()))?;
        Ok(())
    }

    /// True if a host with this name is already registered.
    pub fn has_host(&self, host: &str) -> bool {
        self.hosts.iter().any(|h| h.host == host)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn roundtrip_docker_mode_manifest() {
        let dir = TempDir::new().unwrap();
        let m = ProjectManifest {
            schema_version: PROJECT_MANIFEST_VERSION,
            slug: "spatiebalk".into(),
            mode: ProjectMode::Docker,
            hosts: vec![
                HostEntry {
                    host: "spatiebalk.test".into(),
                    service: Some("laravel.test".into()),
                    port: Some(80),
                    target: None,
                    flags: vec![],
                },
                HostEntry {
                    host: "vite.spatiebalk.test".into(),
                    service: Some("laravel.test".into()),
                    port: Some(5173),
                    target: None,
                    flags: vec!["vite".into()],
                },
            ],
        };
        m.save(dir.path()).unwrap();
        let loaded = ProjectManifest::load(dir.path()).unwrap().unwrap();
        assert_eq!(loaded, m);
    }

    #[test]
    fn host_mode_omits_service_and_port() {
        let dir = TempDir::new().unwrap();
        let m = ProjectManifest {
            schema_version: PROJECT_MANIFEST_VERSION,
            slug: "sparkle".into(),
            mode: ProjectMode::Host,
            hosts: vec![HostEntry {
                host: "sparkle.test".into(),
                service: None,
                port: None,
                target: Some("http://host.docker.internal:3000".into()),
                flags: vec![],
            }],
        };
        m.save(dir.path()).unwrap();
        let loaded = ProjectManifest::load(dir.path()).unwrap().unwrap();
        assert_eq!(loaded, m);

        // Verify the on-disk file doesn't carry service/port for host mode.
        let raw = std::fs::read_to_string(dir.path().join(FILENAME)).unwrap();
        assert!(!raw.contains("service ="));
        assert!(!raw.contains("port ="));
        assert!(raw.contains("target ="));
    }
}
