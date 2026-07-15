//! `~/.config/henk/config.toml` — user-facing settings (TLD, port choices,
//! update-check toggle). Created on first `henk init`; readable later by
//! every command.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::consts::{DASHBOARD_PORT, DEFAULT_TLD, DNSMASQ_PORT, HTTP_PORT, HTTPS_PORT};

/// Config file location: `~/.config/henk/config.toml`. Uses the XDG-style
/// path on every platform — see `stack/paths.rs` for the reasoning.
pub fn path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("could not resolve $HOME")?;
    Ok(home.join(".config").join("henk").join("config.toml"))
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Config {
    pub schema_version: u32,
    /// TLD without leading dot, e.g. `"test"` or `"henk"`.
    pub tld: String,
    pub ports: Ports,
    /// Whether `henk update` should check GitHub Releases for newer versions.
    pub update_check: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Ports {
    pub http: u16,
    pub https: u16,
    pub dnsmasq: u16,
    pub dashboard: u16,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: 1,
            tld: DEFAULT_TLD.to_string(),
            ports: Ports {
                http: HTTP_PORT,
                https: HTTPS_PORT,
                dnsmasq: DNSMASQ_PORT,
                dashboard: DASHBOARD_PORT,
            },
            update_check: true,
        }
    }
}

impl Config {
    /// Load config from `~/.config/henk/config.toml`. Returns `None` if the
    /// file doesn't exist (henk hasn't been init'd yet).
    pub fn load() -> Result<Option<Self>> {
        let p = path()?;
        if !p.exists() {
            return Ok(None);
        }
        let contents =
            fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
        let cfg: Config = toml::from_str(&contents)
            .with_context(|| format!("parsing {} as TOML", p.display()))?;
        Ok(Some(cfg))
    }

    /// Persist config to `~/.config/henk/config.toml`. Atomic (temp + rename).
    pub fn save(&self) -> Result<()> {
        let p = path()?;
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let body = toml::to_string_pretty(self).context("serialising config")?;
        let header = format!(
            "# managed by henk — see https://github.com/yelsed/henk\n# schema_version = {}\n",
            self.schema_version
        );
        let full = format!("{header}\n{body}");
        let tmp = p.with_extension("tmp");
        fs::write(&tmp, full).with_context(|| format!("writing {}", tmp.display()))?;
        fs::rename(&tmp, &p)
            .with_context(|| format!("renaming {} -> {}", tmp.display(), p.display()))?;
        Ok(())
    }

    /// Load existing config or build a default one based on the chosen TLD
    /// (e.g. derived from detection at init time).
    pub fn load_or_init(tld: &str) -> Result<Self> {
        if let Some(existing) = Self::load()? {
            return Ok(existing);
        }
        let cfg = Self {
            tld: tld.trim_start_matches('.').to_ascii_lowercase(),
            ..Self::default()
        };
        cfg.save()?;
        Ok(cfg)
    }
}
