//! Evidence-first project detection for `henk link`.
//!
//! Reads the project's compose, `.env`, and `package.json` to figure
//! out (without prompting where possible):
//!   - whether this is a Docker-mode or Host-mode project,
//!   - which service is the web entry point,
//!   - what container port to route to,
//!   - what hostname to default to,
//!   - whether the service collides with our :80 / :443 binding,
//!   - whether Vite is in play (so we can offer a sub-host).

use anyhow::Result;
use regex::Regex;
use std::fs;
use std::path::Path;

use crate::project::compose::{self, ComposeFile, ComposeService, PublishedPort};
use crate::project::env_file;
use crate::project::manifest::ProjectMode;

#[derive(Debug, Clone)]
pub struct ProjectDetection {
    pub mode: ProjectMode,
    /// `Some` when detection is unambiguous; `None` when the wizard
    /// should prompt with `candidates`.
    pub web_service: Option<String>,
    pub web_port: Option<u16>,
    /// Default hostname suggestion (without https://). Always set.
    pub default_host: String,
    /// All web-eligible services (datastores excluded). For prompting.
    pub candidates: Vec<ServiceCandidate>,
    /// True if the chosen service publishes :80 or :443 on the host —
    /// the user will need to free them or shift `APP_PORT`.
    pub port_collision: Option<u16>,
    /// True if we found Vite in package.json or a vite.config.*.
    pub vite_detected: bool,
}

#[derive(Debug, Clone)]
pub struct ServiceCandidate {
    pub name: String,
    pub container_port: u16,
    /// Host port if published; same as container_port if not.
    pub host_port: u16,
    /// Why we ranked this service: e.g. "matches APP_URL port",
    /// "has published web port :8055", "no other candidates".
    pub rationale: String,
}

/// Run detection against a project at `project_dir`. Slug + TLD are
/// caller-supplied (slug = directory name, tld from config).
pub fn detect(project_dir: &Path, slug: &str, tld: &str) -> Result<ProjectDetection> {
    let env = env_file::read(project_dir)?;
    let compose_path = compose::find_compose_path(project_dir);
    let vite_detected = detect_vite(project_dir);

    if let Some(path) = compose_path {
        let cf = compose::read(&path)?;
        return Ok(detect_docker_mode(&cf, &env, slug, tld, vite_detected));
    }

    // No compose file → host mode (Sparkle-style).
    let port = detect_host_mode_port(&env, project_dir);
    let default_host = default_host_for(&env, slug, tld);
    Ok(ProjectDetection {
        mode: ProjectMode::Host,
        web_service: None,
        web_port: Some(port),
        default_host,
        candidates: Vec::new(),
        port_collision: None,
        vite_detected,
    })
}

fn detect_docker_mode(
    cf: &ComposeFile,
    env: &std::collections::BTreeMap<String, String>,
    slug: &str,
    tld: &str,
    vite_detected: bool,
) -> ProjectDetection {
    // 1. Build the list of web-eligible services (datastores excluded).
    let mut candidates: Vec<ServiceCandidate> = Vec::new();
    for (name, svc) in &cf.services {
        if svc.looks_like_datastore(name) {
            continue;
        }
        if let Some(p) = pick_web_port(svc) {
            candidates.push(ServiceCandidate {
                name: name.clone(),
                container_port: p.container,
                host_port: p.host,
                rationale: rationale_for(name, &p, &cf.services),
            });
        }
    }

    // 2. Try to pin one using `.env` URLs (Laravel APP_URL, Directus
    //    PUBLIC_URL, etc.). If APP_URL points at port :8055, pick the
    //    candidate publishing :8055.
    let env_port = env_url_port(env);
    let chosen = if let Some(target_port) = env_port {
        candidates
            .iter()
            .find(|c| c.host_port == target_port || c.container_port == target_port)
            .cloned()
    } else if candidates.len() == 1 {
        Some(candidates[0].clone())
    } else {
        None
    };

    let port_collision = chosen.as_ref().and_then(|c| {
        if c.host_port == 80 || c.host_port == 443 {
            Some(c.host_port)
        } else {
            None
        }
    });

    let default_host = default_host_for(env, slug, tld);

    ProjectDetection {
        mode: ProjectMode::Docker,
        web_service: chosen.as_ref().map(|c| c.name.clone()),
        web_port: chosen.as_ref().map(|c| c.container_port),
        default_host,
        candidates,
        port_collision,
        vite_detected,
    }
}

/// Pick the most "web-ish" published port from a service's port list.
/// Preference order is `consts::WEB_PORTS`. If the service publishes no
/// ports at all, returns `None` and the service is excluded as a
/// candidate (we can't reach it anyway).
fn pick_web_port(svc: &ComposeService) -> Option<PublishedPort> {
    let ports = svc.published_ports();
    if ports.is_empty() {
        return None;
    }
    for &preferred in crate::consts::WEB_PORTS {
        if let Some(p) = ports.iter().find(|p| p.container == preferred) {
            return Some(*p);
        }
    }
    Some(ports[0])
}

fn rationale_for(
    name: &str,
    p: &PublishedPort,
    _services: &std::collections::BTreeMap<String, ComposeService>,
) -> String {
    format!(
        "`{name}` publishes :{} (container :{})",
        p.host, p.container
    )
}

/// Extract the port number from `.env` URL-shaped values:
/// APP_URL, PUBLIC_URL, NUXT_BASE_URL, APP_BASE_URL.
fn env_url_port(env: &std::collections::BTreeMap<String, String>) -> Option<u16> {
    for key in ["APP_URL", "PUBLIC_URL", "NUXT_BASE_URL", "APP_BASE_URL"] {
        if let Some(url) = env.get(key)
            && let Some(port) = port_from_url(url)
        {
            return Some(port);
        }
    }
    None
}

fn port_from_url(url: &str) -> Option<u16> {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let host = after_scheme.split('/').next().unwrap_or(after_scheme);
    if let Some((_, port)) = host.rsplit_once(':') {
        port.parse::<u16>().ok()
    } else {
        // No explicit port — assume :80 for http, :443 for https.
        if url.starts_with("https://") {
            Some(443)
        } else {
            Some(80)
        }
    }
}

/// Default-hostname rule:
/// 1. If `.env` has an `*_URL` whose host part ends with the chosen TLD,
///    use the host part directly (e.g. `APP_URL=http://app.test` →
///    `app.test`).
/// 2. Otherwise, `<slug>.<tld>`.
fn default_host_for(
    env: &std::collections::BTreeMap<String, String>,
    slug: &str,
    tld: &str,
) -> String {
    for key in ["APP_URL", "PUBLIC_URL", "NUXT_BASE_URL", "APP_BASE_URL"] {
        if let Some(url) = env.get(key)
            && let Some(host) = host_from_url(url)
        {
            let bare = host.split(':').next().unwrap_or("");
            let tld_suffix = format!(".{tld}");
            if bare.ends_with(&tld_suffix) && !bare.is_empty() {
                return bare.to_string();
            }
        }
    }
    format!("{slug}.{tld}")
}

fn host_from_url(url: &str) -> Option<&str> {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let host = after_scheme.split('/').next()?;
    if host.is_empty() { None } else { Some(host) }
}

/// Host-mode port detection: read `.env` `*_URL`s and `package.json`
/// scripts for a port hint. Falls back to 3000 (Nuxt/Next default).
fn detect_host_mode_port(
    env: &std::collections::BTreeMap<String, String>,
    project_dir: &Path,
) -> u16 {
    if let Some(p) = env_url_port(env) {
        return p;
    }
    if let Ok(body) = fs::read_to_string(project_dir.join("package.json"))
        && let Some(p) = port_from_package_json(&body)
    {
        return p;
    }
    3000
}

fn port_from_package_json(body: &str) -> Option<u16> {
    // Look for `--port 1234` or `--port=1234` in scripts.
    let re = Regex::new(r"--port[ =](\d{2,5})").ok()?;
    re.captures(body)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse::<u16>().ok())
}

/// `package.json` lists `vite` or `@vitejs/plugin-*`, OR a
/// `vite.config.*` exists at the project root.
fn detect_vite(project_dir: &Path) -> bool {
    for name in ["vite.config.js", "vite.config.ts", "vite.config.mjs"] {
        if project_dir.join(name).exists() {
            return true;
        }
    }
    if let Ok(body) = fs::read_to_string(project_dir.join("package.json")) {
        return body.contains("\"vite\"") || body.contains("\"@vitejs/plugin-");
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn extracts_port_from_app_url_with_port() {
        let mut env = BTreeMap::new();
        env.insert("APP_URL".into(), "http://localhost:8055".into());
        assert_eq!(env_url_port(&env), Some(8055));
    }

    #[test]
    fn defaults_port_to_80_for_unportforhttp_url() {
        let mut env = BTreeMap::new();
        env.insert("APP_URL".into(), "http://localhost".into());
        assert_eq!(env_url_port(&env), Some(80));
    }

    #[test]
    fn defaults_host_to_slug_when_no_test_url() {
        let env = BTreeMap::new();
        assert_eq!(
            default_host_for(&env, "spatiebalk", "test"),
            "spatiebalk.test"
        );
    }

    #[test]
    fn host_default_picks_test_url_host_when_present() {
        let mut env = BTreeMap::new();
        env.insert("APP_URL".into(), "http://customname.test".into());
        assert_eq!(default_host_for(&env, "ignored", "test"), "customname.test");
    }

    #[test]
    fn vite_detected_via_package_json() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"devDependencies":{"vite":"^7.0.0"}}"#,
        )
        .unwrap();
        assert!(detect_vite(dir.path()));
    }

    #[test]
    fn vite_detected_via_config_file() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("vite.config.ts"), "export default {}").unwrap();
        assert!(detect_vite(dir.path()));
    }

    #[test]
    fn host_mode_falls_back_to_3000_when_nothing_known() {
        let dir = tempfile::TempDir::new().unwrap();
        let env = BTreeMap::new();
        assert_eq!(detect_host_mode_port(&env, dir.path()), 3000);
    }

    #[test]
    fn package_json_port_extracted_from_dev_script() {
        let body = r#"{"scripts":{"dev":"next dev --port 4200"}}"#;
        assert_eq!(port_from_package_json(body), Some(4200));
    }
}
