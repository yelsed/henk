//! Render the global stack config (Traefik + dnsmasq) to
//! `~/.config/henk/traefik/`.
//!
//! Templates are embedded via `include_str!` and substituted using a tiny
//! `{{NAME}}` syntax. We deliberately avoid pulling in a templating engine
//! while substitutions are this trivial; switch to `minijinja` if templates
//! grow.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::config::Config;
use crate::consts::HENK_FILE_HEADER;
use crate::stack::paths;

const COMPOSE_TMPL: &str = include_str!("../../assets/traefik/compose.yml.tmpl");
const TRAEFIK_TMPL: &str = include_str!("../../assets/traefik/traefik.yml.tmpl");
const DYNAMIC_TMPL: &str = include_str!("../../assets/traefik/dynamic.yml.tmpl");

/// Substitution variables shared across the templates.
fn vars_from(cfg: &Config) -> BTreeMap<&'static str, String> {
    let mut vars = BTreeMap::new();
    vars.insert("HENK_FILE_HEADER", HENK_FILE_HEADER.to_string());
    vars.insert("HTTP_PORT", cfg.ports.http.to_string());
    vars.insert("HTTPS_PORT", cfg.ports.https.to_string());
    vars.insert("DASHBOARD_PORT", cfg.ports.dashboard.to_string());
    vars.insert("TLD", cfg.tld.clone());
    vars
}

fn render(template: &str, vars: &BTreeMap<&'static str, String>) -> String {
    let mut out = template.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("{{{{{k}}}}}"), v);
    }
    out
}

/// Write all stack-config files into `~/.config/henk/traefik/`.
/// Idempotent: if the existing file matches what we'd write, leaves it
/// untouched. Atomic per-file (temp + rename).
pub fn render_all(cfg: &Config) -> Result<()> {
    let vars = vars_from(cfg);
    let traefik_dir = paths::traefik_dir()?;
    fs::create_dir_all(&traefik_dir)
        .with_context(|| format!("creating {}", traefik_dir.display()))?;
    fs::create_dir_all(traefik_dir.join("certs"))
        .with_context(|| format!("creating {}", traefik_dir.join("certs").display()))?;

    write_if_changed(
        &paths::traefik_compose_path()?,
        &render(COMPOSE_TMPL, &vars),
    )?;
    write_if_changed(&paths::traefik_static_path()?, &render(TRAEFIK_TMPL, &vars))?;
    write_if_changed(
        &paths::traefik_dynamic_path()?,
        &render(DYNAMIC_TMPL, &vars),
    )?;
    // dnsmasq.conf is no longer rendered into the compose dir — dnsmasq runs
    // under Homebrew/launchd on the host (M3.5). See `stack/dnsmasq.rs`.

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
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, contents).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config::default()
    }

    #[test]
    fn compose_template_substitutes_ports_and_header() {
        let rendered = render(COMPOSE_TMPL, &vars_from(&cfg()));
        assert!(rendered.contains("# managed by henk"));
        assert!(rendered.contains("\"80:80\""));
        assert!(rendered.contains("\"443:443\""));
        assert!(rendered.contains("127.0.0.1:19080:8080"));
        assert!(rendered.contains("name: henk-proxy"));
        assert!(
            !rendered.contains("{{"),
            "no template residue: \n{rendered}"
        );
    }

    #[test]
    fn traefik_template_uses_file_provider_only() {
        // M3 architecture: docker provider intentionally absent (Docker 29.x
        // rejects Traefik's hardcoded /v1.24 API calls). henk maintains
        // dynamic.yml directly. See traefik.yml.tmpl comment for context.
        let rendered = render(TRAEFIK_TMPL, &vars_from(&cfg()));
        assert!(rendered.contains("file:"), "needs file provider");
        assert!(
            !rendered.contains("docker:"),
            "must NOT enable docker provider"
        );
        assert!(!rendered.contains("{{"));
    }

    #[test]
    fn dynamic_template_carries_wildcard_cert_paths() {
        let rendered = render(DYNAMIC_TMPL, &vars_from(&cfg()));
        assert!(rendered.contains("certFile: /certs/_wildcard.test.pem"));
        assert!(rendered.contains("keyFile: /certs/_wildcard.test-key.pem"));
        assert!(rendered.contains("# managed by henk"));
        assert!(!rendered.contains("{{"));
    }

    #[test]
    fn fallback_tld_substitutes_throughout() {
        let mut cfg = Config::default();
        cfg.tld = "henk".into();
        let v = vars_from(&cfg);
        // Dashboard router rule lives in dynamic.yml (M3 file-provider-only
        // architecture). Cert paths follow the chosen TLD.
        assert!(render(DYNAMIC_TMPL, &v).contains("Host(`traefik.henk`)"));
        assert!(render(DYNAMIC_TMPL, &v).contains("certFile: /certs/_wildcard.henk.pem"));
    }
}
