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

/// `~/.config/henk/traefik/dynamic.yml`.
pub fn traefik_dynamic_path() -> Result<PathBuf> {
    Ok(traefik_dir()?.join("dynamic.yml"))
}

/// `~/.config/henk/dynamic/` — Traefik file-provider entries for Host-mode
/// projects. Empty in M2; populated by `henk link` in M6.
#[allow(dead_code)]
pub fn dynamic_projects_dir() -> Result<PathBuf> {
    Ok(config_dir()?.join("dynamic"))
}
