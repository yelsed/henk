//! Filesystem paths for henk's global state.
//!
//! All paths are XDG-style under `~/.config/henk/` on every platform we
//! support (macOS only for now). We deliberately do NOT use
//! `dirs::config_dir()` because on macOS that returns
//! `~/Library/Application Support/`, which is the GUI convention; CLI
//! tools idiomatically live under `~/.config/`.

use anyhow::{Context, Result};
use std::path::PathBuf;

/// `~/.config/henk/`. Created on demand.
pub fn config_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("could not resolve $HOME")?;
    Ok(home.join(".config").join("henk"))
}

/// `~/.config/henk/traefik/`. The directory the global compose stack lives in.
pub fn traefik_dir() -> Result<PathBuf> {
    Ok(config_dir()?.join("traefik"))
}

/// `~/.config/henk/traefik/compose.yml`.
pub fn traefik_compose_path() -> Result<PathBuf> {
    Ok(traefik_dir()?.join("compose.yml"))
}

/// `~/.config/henk/traefik/traefik.yml`.
pub fn traefik_static_path() -> Result<PathBuf> {
    Ok(traefik_dir()?.join("traefik.yml"))
}

/// `~/.config/henk/dynamic/` — Traefik file-provider directory.
/// Holds the dashboard / TLS YAML (`_henk.yml`) plus one file per
/// linked project (`<slug>.yml`). Mounted into Traefik via the
/// compose file in `directory:` mode.
pub fn dynamic_projects_dir() -> Result<PathBuf> {
    Ok(config_dir()?.join("dynamic"))
}

/// Path to the dashboard / TLS file inside the dynamic dir.
pub fn traefik_dynamic_path() -> Result<PathBuf> {
    Ok(dynamic_projects_dir()?.join("_henk.yml"))
}

/// `~/.config/henk/errorpages/` — the pages served when a request can't reach a
/// healthy backend. Mounted into the error-pages container as its web root.
pub fn errorpages_dir() -> Result<PathBuf> {
    Ok(config_dir()?.join("errorpages"))
}

/// A file inside `~/.config/henk/errorpages/`, e.g. `down.html`, `down.txt`.
pub fn errorpage_path(file: &str) -> Result<PathBuf> {
    Ok(errorpages_dir()?.join(file))
}

/// `~/.config/henk/errorpages/nginx.conf`.
pub fn errorpage_nginx_path() -> Result<PathBuf> {
    errorpage_path("nginx.conf")
}
