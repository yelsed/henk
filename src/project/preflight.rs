//! Static project analysis surfaced after `henk link`.
//!
//! These checks read the user's project files (vite/nuxt config,
//! package.json, .env, compose) and look for the known gotchas that
//! make `https://<slug>.<tld>` go 502 or render a blank page despite
//! routing being correct. Printed as warnings — never blockers — so
//! users can copy-paste the fix without re-running `henk link`.
//!
//! All checks are intentionally pattern-based rather than fully
//! parsing TS/JS; that's good enough to catch missing `server.host`
//! or `cors.origin` in a vite.config without dragging in a JS
//! parser. False positives (config split across multiple files,
//! exotic syntax) print a generic recommendation instead of a
//! narrow assertion.

use crate::project::detect::ProjectDetection;
use crate::project::manifest::ProjectMode;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    High,
    Medium,
}

#[derive(Debug, Clone)]
pub struct Issue {
    pub severity: Severity,
    pub title: String,
    pub explain: String,
    /// Target file/.env key the user should edit, for human guidance.
    pub fix_target: String,
    /// The exact snippet / line to paste.
    pub snippet: String,
}

/// Run every applicable check against `project_dir`. `default_host` is
/// used to tailor framework snippets (e.g. CORS origin matches the
/// app host the user just linked).
pub fn analyze(
    project_dir: &Path,
    detection: &ProjectDetection,
    default_host: &str,
    vite_host_added: Option<&str>,
) -> Vec<Issue> {
    let mut out = Vec::new();
    let env = read_env(project_dir);
    let pkg = read_package_json(project_dir);

    if detection.vite_detected || vite_host_added.is_some() {
        out.extend(check_vite_config(project_dir, default_host));
        if vite_host_added.is_some() && project_dir.join("vendor").join("bin").join("sail").exists()
        {
            out.extend(check_sail_vite_port_collision(env.as_deref()));
        }
    }

    if matches!(detection.mode, ProjectMode::Host) {
        out.extend(check_nuxt_devserver(project_dir));
        out.extend(check_dev_script_bind_flag(pkg.as_deref(), project_dir));
    }

    out
}

fn check_vite_config(project_dir: &Path, default_host: &str) -> Vec<Issue> {
    let mut out = Vec::new();
    let cfg = read_vite_config(project_dir);
    let Some((path, body)) = cfg else { return out };

    let lower = body.to_ascii_lowercase();
    let has_host = lower.contains("host:") && lower.contains("0.0.0.0");
    let has_cors = lower.contains("cors:");

    if !has_host {
        out.push(Issue {
            severity: Severity::High,
            title: "Vite is likely binding 127.0.0.1 only".into(),
            explain:
                "Without `server.host: '0.0.0.0'`, Vite binds the IPv6/v4 \
                 loopback only. host.docker.internal can't reach it from \
                 the henk-traefik container — you'll get 502."
                    .into(),
            fix_target: format!("edit {}", path.display()),
            snippet: format!(
                "server: {{\n  host: '0.0.0.0',\n  port: 5173,\n  strictPort: true,\n  hmr: {{ host: 'vite.{default_host}', protocol: 'wss', clientPort: 443 }},\n  origin: 'https://vite.{default_host}',\n  cors: {{ origin: 'https://{default_host}' }},\n}}"
            ),
        });
    }
    if !has_cors {
        out.push(Issue {
            severity: Severity::High,
            title: "Vite v7 CORS will block cross-subdomain module loads".into(),
            explain: "Vite v7 sets Access-Control-Allow-Origin to its own \
                 `origin`, not the requesting page's. Module imports from \
                 the app host get blocked, leaving a blank page even \
                 though the assets are reachable."
                .into(),
            fix_target: format!("edit {}", path.display()),
            snippet: format!(
                "// inside `server: {{ ... }}`\ncors: {{ origin: 'https://{default_host}' }},"
            ),
        });
    }
    out
}

fn check_sail_vite_port_collision(env_body: Option<&str>) -> Vec<Issue> {
    let mut out = Vec::new();
    let port_publishing_5173 = match env_body {
        Some(body) => {
            // VITE_PORT unset OR =5173 means Sail publishes 5173:5173.
            let line = body.lines().find(|l| {
                let l = l.trim();
                !l.starts_with('#') && l.starts_with("VITE_PORT")
            });
            match line {
                Some(l) => {
                    let val = l
                        .split('=')
                        .nth(1)
                        .unwrap_or("5173")
                        .trim()
                        .trim_matches('"');
                    val == "5173"
                }
                None => true, // unset → defaults to 5173
            }
        }
        None => true,
    };
    if port_publishing_5173 {
        out.push(Issue {
            severity: Severity::Medium,
            title: "Sail's compose publishes :5173 — Vite on host can't bind it".into(),
            explain: "docker-proxy reserves the host port even when the \
                 container has no Vite process inside. Run Vite on the \
                 host (the conventional Sail flow) by giving Sail a \
                 different port to publish."
                .into(),
            fix_target: "append to .env".into(),
            snippet: "VITE_PORT=15173".into(),
        });
    }
    out
}

fn check_nuxt_devserver(project_dir: &Path) -> Vec<Issue> {
    let Some((path, body)) = read_nuxt_config(project_dir) else {
        return Vec::new();
    };
    let lower = body.to_ascii_lowercase();
    if lower.contains("devserver") {
        return Vec::new();
    }
    vec![Issue {
        severity: Severity::High,
        title: "Nuxt likely binds [::1] only — host mode will 502".into(),
        explain: "Nuxt's default dev server binds the IPv6 loopback. \
             host.docker.internal only sees ports on the IPv4 wildcard, \
             so the Traefik container can't reach it. Set `devServer.host` \
             explicitly."
            .into(),
        fix_target: format!("edit {}", path.display()),
        snippet: "devServer: { host: '0.0.0.0', port: 3000 }".into(),
    }]
}

fn check_dev_script_bind_flag(pkg_body: Option<&str>, project_dir: &Path) -> Vec<Issue> {
    // Skip when nuxt.config sets devServer — that supersedes the CLI flag.
    if let Some((_, body)) = read_nuxt_config(project_dir) {
        if body.to_ascii_lowercase().contains("devserver") {
            return Vec::new();
        }
    }
    let Some(body) = pkg_body else {
        return Vec::new();
    };
    // Extract `"dev": "..."` value, regardless of pretty-printing or not.
    use std::sync::LazyLock;
    static RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r#""dev"\s*:\s*"([^"]+)""#).expect("static regex"));
    let dev_script = RE
        .captures(body)
        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
        .unwrap_or_default();
    if dev_script.is_empty() {
        return Vec::new();
    }
    if dev_script.contains("--host 0.0.0.0") || dev_script.contains("--host=0.0.0.0") {
        return Vec::new();
    }
    vec![Issue {
        severity: Severity::Medium,
        title: "`npm run dev` doesn't pass --host 0.0.0.0".into(),
        explain: "Without it, the dev server may bind localhost only and \
             host-mode routing fails. Either edit the script or set \
             devServer/host in the framework config."
            .into(),
        fix_target: "package.json scripts.dev".into(),
        snippet: "\"dev\": \"<existing-runner> --host 0.0.0.0 --port 3000\"".into(),
    }]
}

// ── readers ────────────────────────────────────────────────────────────────

fn read_env(project_dir: &Path) -> Option<String> {
    for name in [".env.local", ".env"] {
        let p = project_dir.join(name);
        if let Ok(body) = fs::read_to_string(&p) {
            return Some(body);
        }
    }
    None
}

fn read_package_json(project_dir: &Path) -> Option<String> {
    fs::read_to_string(project_dir.join("package.json")).ok()
}

fn read_vite_config(project_dir: &Path) -> Option<(PathBuf, String)> {
    for name in ["vite.config.ts", "vite.config.js", "vite.config.mjs"] {
        let p = project_dir.join(name);
        if let Ok(body) = fs::read_to_string(&p) {
            return Some((p, body));
        }
    }
    None
}

fn read_nuxt_config(project_dir: &Path) -> Option<(PathBuf, String)> {
    for name in ["nuxt.config.ts", "nuxt.config.js", "nuxt.config.mjs"] {
        let p = project_dir.join(name);
        if let Ok(body) = fs::read_to_string(&p) {
            return Some((p, body));
        }
    }
    None
}

// ── printing ───────────────────────────────────────────────────────────────

pub fn print_report(issues: &[Issue]) {
    use owo_colors::OwoColorize;
    if issues.is_empty() {
        return;
    }
    println!();
    println!("{}", "Preflight checks".bold());
    println!();
    for issue in issues {
        let badge = match issue.severity {
            Severity::High => "[high]".red().to_string(),
            Severity::Medium => "[med] ".yellow().to_string(),
        };
        println!("  {} {}", badge, issue.title.bold());
        println!("        {}", issue.explain);
        println!("        → {}", issue.fix_target);
        for line in issue.snippet.lines() {
            println!("            {line}");
        }
        println!();
    }
    println!(
        "  {} fixes are non-destructive — paste, restart your dev server, you're done.",
        "i".bright_black()
    );
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::detect::ProjectDetection;
    use crate::project::manifest::ProjectMode;
    use tempfile::TempDir;

    fn host_detection() -> ProjectDetection {
        ProjectDetection {
            mode: ProjectMode::Host,
            web_service: None,
            web_port: Some(3000),
            default_host: "sparkle.test".into(),
            candidates: vec![],
            port_collision: None,
            vite_detected: false,
        }
    }

    fn docker_vite_detection() -> ProjectDetection {
        ProjectDetection {
            mode: ProjectMode::Docker,
            web_service: Some("laravel.test".into()),
            web_port: Some(80),
            default_host: "spatiebalk.test".into(),
            candidates: vec![],
            port_collision: None,
            vite_detected: true,
        }
    }

    #[test]
    fn vite_config_without_host_or_cors_flags_both() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("vite.config.js"),
            "export default defineConfig({ plugins: [] });",
        )
        .unwrap();
        let issues = analyze(
            dir.path(),
            &docker_vite_detection(),
            "spatiebalk.test",
            None,
        );
        assert!(issues.iter().any(|i| i.title.contains("binding 127.0.0.1")));
        assert!(issues.iter().any(|i| i.title.contains("CORS")));
    }

    #[test]
    fn vite_config_with_host_but_no_cors_flags_cors_only() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("vite.config.js"),
            "export default defineConfig({ server: { host: '0.0.0.0' } });",
        )
        .unwrap();
        let issues = analyze(
            dir.path(),
            &docker_vite_detection(),
            "spatiebalk.test",
            None,
        );
        assert!(!issues.iter().any(|i| i.title.contains("binding 127.0.0.1")));
        assert!(issues.iter().any(|i| i.title.contains("CORS")));
    }

    #[test]
    fn nuxt_config_without_devserver_flags_high() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("nuxt.config.ts"),
            "export default defineNuxtConfig({ compatibilityDate: '2025-07-15' });",
        )
        .unwrap();
        let issues = analyze(dir.path(), &host_detection(), "sparkle.test", None);
        assert!(issues.iter().any(|i| i.title.contains("Nuxt likely binds")));
    }

    #[test]
    fn nuxt_config_with_devserver_doesnt_flag() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("nuxt.config.ts"),
            "export default defineNuxtConfig({ devServer: { host: '0.0.0.0' } });",
        )
        .unwrap();
        let issues = analyze(dir.path(), &host_detection(), "sparkle.test", None);
        assert!(!issues.iter().any(|i| i.title.contains("Nuxt likely binds")));
    }

    #[test]
    fn sail_vite_port_default_flagged() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("vendor").join("bin")).unwrap();
        std::fs::write(
            dir.path().join("vendor").join("bin").join("sail"),
            "#!/bin/sh\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("vite.config.js"),
            "export default defineConfig({ server: { host: '0.0.0.0', cors: { origin: 'x' } } });",
        )
        .unwrap();
        std::fs::write(dir.path().join(".env"), "APP_NAME=test\n").unwrap();
        let issues = analyze(
            dir.path(),
            &docker_vite_detection(),
            "spatiebalk.test",
            Some("vite.spatiebalk.test"),
        );
        assert!(
            issues
                .iter()
                .any(|i| i.title.contains("Sail's compose publishes :5173"))
        );
    }

    #[test]
    fn sail_vite_port_set_to_alt_passes() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("vendor").join("bin")).unwrap();
        std::fs::write(dir.path().join("vendor").join("bin").join("sail"), "").unwrap();
        std::fs::write(
            dir.path().join("vite.config.js"),
            "export default defineConfig({ server: { host: '0.0.0.0', cors: { origin: 'x' } } });",
        )
        .unwrap();
        std::fs::write(dir.path().join(".env"), "VITE_PORT=15173\n").unwrap();
        let issues = analyze(
            dir.path(),
            &docker_vite_detection(),
            "spatiebalk.test",
            Some("vite.spatiebalk.test"),
        );
        assert!(
            !issues
                .iter()
                .any(|i| i.title.contains("Sail's compose publishes :5173"))
        );
    }

    #[test]
    fn dev_script_with_host_flag_doesnt_flag() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{ "scripts": { "dev": "next dev --host 0.0.0.0 --port 3000" } }"#,
        )
        .unwrap();
        let issues = analyze(dir.path(), &host_detection(), "sparkle.test", None);
        assert!(!issues.iter().any(|i| i.title.contains("--host 0.0.0.0")));
    }

    #[test]
    fn dev_script_without_host_flag_flagged_when_no_nuxt_config() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{ "scripts": { "dev": "next dev" } }"#,
        )
        .unwrap();
        let issues = analyze(dir.path(), &host_detection(), "sparkle.test", None);
        assert!(issues.iter().any(|i| i.title.contains("--host 0.0.0.0")));
    }
}
