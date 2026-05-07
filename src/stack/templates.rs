//! Render the global Traefik stack config to `~/.config/henk/traefik/`.
//!
//! Templates are embedded via `include_str!` and substituted using a tiny
//! `{{NAME}}` syntax. We deliberately avoid pulling in a templating engine
//! while substitutions are this trivial; switch to `minijinja` if templates
//! grow.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::consts::{DASHBOARD_PORT, HENK_FILE_HEADER, HTTP_PORT, HTTPS_PORT};
use crate::stack::paths;

const COMPOSE_TMPL: &str = include_str!("../../assets/traefik/compose.yml.tmpl");
const TRAEFIK_TMPL: &str = include_str!("../../assets/traefik/traefik.yml.tmpl");
const DYNAMIC_TMPL: &str = include_str!("../../assets/traefik/dynamic.yml.tmpl");

/// Substitution variables shared across the templates.
fn default_vars() -> BTreeMap<&'static str, String> {
    let mut vars = BTreeMap::new();
    vars.insert("HENK_FILE_HEADER", HENK_FILE_HEADER.to_string());
    vars.insert("HTTP_PORT", HTTP_PORT.to_string());
    vars.insert("HTTPS_PORT", HTTPS_PORT.to_string());
    vars.insert("DASHBOARD_PORT", DASHBOARD_PORT.to_string());
    vars
}

fn render(template: &str, vars: &BTreeMap<&'static str, String>) -> String {
    let mut out = template.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("{{{{{k}}}}}"), v);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_template_substitutes_ports_and_header() {
        let vars = default_vars();
        let rendered = render(COMPOSE_TMPL, &vars);
        assert!(
            rendered.contains("# managed by henk"),
            "compose.yml should carry the henk header"
        );
        assert!(
            rendered.contains(&format!("\"{HTTP_PORT}:80\"")),
            "compose.yml should bind host {HTTP_PORT} to container 80"
        );
        assert!(
            rendered.contains(&format!("\"{HTTPS_PORT}:443\"")),
            "compose.yml should bind host {HTTPS_PORT} to container 443"
        );
        assert!(
            rendered.contains(&format!("127.0.0.1:{DASHBOARD_PORT}:8080")),
            "compose.yml should bind dashboard on loopback only"
        );
        assert!(
            !rendered.contains("{{"),
            "no unsubstituted template tokens may remain: \n{rendered}"
        );
    }

    #[test]
    fn traefik_template_has_docker_provider() {
        // M2 only wires up the docker provider; the file provider lands in M3
        // when dynamic.yml gains TLS material.
        let vars = default_vars();
        let rendered = render(TRAEFIK_TMPL, &vars);
        assert!(rendered.contains("docker:"), "needs docker provider");
        assert!(rendered.contains("network: henk-proxy"));
        assert!(!rendered.contains("{{"));
    }

    #[test]
    fn dynamic_template_carries_header_and_no_template_residue() {
        // M2: dynamic.yml is intentionally just a comment header. M3 adds TLS
        // material; M6 adds host-mode routers/services.
        let vars = default_vars();
        let rendered = render(DYNAMIC_TMPL, &vars);
        assert!(rendered.contains("# managed by henk"));
        assert!(!rendered.contains("{{"));
    }
}

/// Write the three Traefik config files into `~/.config/henk/traefik/`.
/// Idempotent: if the existing file matches what we'd write, leaves it
/// untouched. Atomic per-file (temp + rename).
pub fn render_all() -> Result<()> {
    let vars = default_vars();
    let traefik_dir = paths::traefik_dir()?;
    fs::create_dir_all(&traefik_dir)
        .with_context(|| format!("creating {}", traefik_dir.display()))?;

    write_if_changed(&paths::traefik_compose_path()?, &render(COMPOSE_TMPL, &vars))?;
    write_if_changed(&paths::traefik_static_path()?, &render(TRAEFIK_TMPL, &vars))?;
    write_if_changed(&paths::traefik_dynamic_path()?, &render(DYNAMIC_TMPL, &vars))?;

    Ok(())
}

fn write_if_changed(path: &Path, contents: &str) -> Result<()> {
    if let Ok(existing) = fs::read_to_string(path) {
        if existing == contents {
            return Ok(());
        }
    }
    let parent = path
        .parent()
        .context("template path must have a parent directory")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("creating {}", parent.display()))?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, contents).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}
