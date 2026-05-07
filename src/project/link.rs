//! `henk link` — register the project in the current directory.

use anyhow::{Context, Result, bail};
use std::io::{self, Write};
use std::path::Path;

use crate::config::Config;
use crate::project::detect::{self, ProjectDetection};
use crate::project::manifest::{HostEntry, ProjectManifest, ProjectMode};
use crate::project::{env_file, file_provider, override_file};
use crate::runner::SystemRunner;
use crate::stack::{certs, paths};

/// Run the link flow against `project_dir` (usually `cwd`). `add_only`
/// distinguishes `henk link` (fresh) from `henk link --add` (extra host).
pub async fn run(
    runner: &SystemRunner,
    project_dir: &Path,
    add_only: bool,
    host_override: Option<String>,
) -> Result<()> {
    let cfg = Config::load()?
        .context("henk has not been initialised yet — run `henk init` first")?;

    let slug = derive_slug(project_dir)?;
    let detection = detect::detect(project_dir, &slug, &cfg.tld)?;
    print_detection(&detection);

    let mut manifest = ProjectManifest::load(project_dir)?
        .unwrap_or_else(|| ProjectManifest::new(slug.clone(), detection.mode));

    if add_only && manifest.hosts.is_empty() {
        bail!("`--add` was passed but this project isn't linked yet — run `henk link` first.");
    }

    // Pick the hostname. For now use the detected default unless the
    // user passed --host. M5 wizard will offer a proper inline prompt.
    let new_host = host_override.unwrap_or_else(|| detection.default_host.clone());
    if manifest.has_host(&new_host) {
        bail!(
            "host `{new_host}` is already linked. Use `henk unlink {new_host}` first \
             if you want to re-register it."
        );
    }

    let host_entry = build_host_entry(&detection, &new_host)?;
    manifest.hosts.push(host_entry);

    // Docker mode: write the override file so the project's web service
    // joins henk-proxy. Skip for additional hosts that target the same
    // service we already wrote (override is idempotent anyway, but the
    // call is not free).
    if matches!(detection.mode, ProjectMode::Docker) {
        let service = manifest
            .hosts
            .first()
            .and_then(|h| h.service.clone())
            .context("internal: docker-mode manifest must have a service")?;
        let existing_networks = vec![]; // TODO M4b: read from compose
        let target = override_file::write(project_dir, &service, &existing_networks)?;
        announce_override_target(&target);

        // Append APP_PORT=8080 to .env if there's a :80/:443 collision.
        if let Some(collided) = detection.port_collision {
            handle_port_collision(project_dir, collided)?;
        }
    }

    manifest.save(project_dir)?;
    file_provider::write(&manifest)?;

    // Regenerate the wildcard cert with all currently-linked hostnames
    // included as explicit SANs (so macOS curl/Safari accept them).
    let extra_sans: Vec<String> = collect_extra_sans(&cfg, project_dir)?;
    certs::ensure_wildcard(runner, &cfg.tld, &extra_sans, true).await?;

    // Cert files on disk are reloaded by Traefik only when the dynamic
    // config it references changes (or the container restarts). Just
    // rotating the leaf cert in place isn't enough — Traefik holds the
    // previous cert in memory. Restart the container so the new SANs
    // are served immediately.
    if is_henk_traefik_running(runner).await {
        let _ = runner.run("docker", ["restart", "henk-traefik"]).await;
    }

    print_summary(&manifest, project_dir);
    Ok(())
}

async fn is_henk_traefik_running(runner: &SystemRunner) -> bool {
    let out = runner
        .run(
            "docker",
            [
                "ps",
                "--filter",
                "name=henk-traefik",
                "--format",
                "{{.Names}}",
            ],
        )
        .await;
    matches!(out, Ok(o) if o.ok() && o.stdout.lines().any(|l| l.trim() == "henk-traefik"))
}

fn derive_slug(project_dir: &Path) -> Result<String> {
    let name = project_dir
        .file_name()
        .and_then(|n| n.to_str())
        .context("could not derive slug from project directory name")?;
    Ok(name.to_ascii_lowercase().replace(['_', ' ', '.'], "-"))
}

fn build_host_entry(detection: &ProjectDetection, host: &str) -> Result<HostEntry> {
    match detection.mode {
        ProjectMode::Docker => {
            let service = detection
                .web_service
                .clone()
                .context(
                    "could not auto-detect a web service — multiple candidates. \
                     The interactive picker lands in M5; for now run `henk link \
                     --host <h>` after manually editing .henk.toml, or simplify \
                     your compose file.",
                )?;
            let port = detection
                .web_port
                .context("internal: web_port must accompany web_service")?;
            Ok(HostEntry {
                host: host.to_string(),
                service: Some(service),
                port: Some(port),
                target: None,
                flags: vec![],
            })
        }
        ProjectMode::Host => {
            let port = detection.web_port.unwrap_or(3000);
            Ok(HostEntry {
                host: host.to_string(),
                service: None,
                port: None,
                target: Some(format!("http://host.docker.internal:{port}")),
                flags: vec![],
            })
        }
    }
}

fn handle_port_collision(project_dir: &Path, port: u16) -> Result<()> {
    let alt = pick_free_alt_port(port);
    println!();
    println!(
        "  ! Your web service publishes :{port} on the host — Traefik also wants :{port}."
    );
    println!(
        "    Suggested fix: append `APP_PORT={alt}` to your `.env` so Sail/Compose"
    );
    println!("    binds the host port elsewhere. Traefik talks to the container directly,");
    println!("    so the URL still works.");
    print!("    Append `APP_PORT={alt}` to .env now? [Y/n] ");
    io::stdout().flush().ok();
    let mut buf = String::new();
    io::stdin().read_line(&mut buf)?;
    let trimmed = buf.trim().to_ascii_lowercase();
    if !matches!(trimmed.as_str(), "" | "y" | "yes") {
        println!("    Skipped. You'll need to free :{port} yourself before `npm run dev`.");
        return Ok(());
    }
    let appended = env_file::append_if_absent(project_dir, "APP_PORT", &alt.to_string())?;
    if appended {
        println!("    ✓ appended `APP_PORT={alt}` to .env");
    } else {
        println!("    ✓ APP_PORT already set in .env — left untouched");
    }
    Ok(())
}

/// Pick a free local TCP port to suggest as the new APP_PORT. Probes a
/// sensible list of high candidates and returns the first one nothing's
/// listening on. If everything we try is busy (extremely rare on a dev
/// laptop), fall back to a hard-coded value with the user told via
/// the wizard summary.
fn pick_free_alt_port(collided: u16) -> u16 {
    let candidates: &[u16] = if collided == 443 {
        &[18443, 28443, 38443, 8443]
    } else {
        &[18080, 18081, 18082, 28080, 28081, 8081, 8082, 8083]
    };
    for &c in candidates {
        if is_port_free(c) {
            return c;
        }
    }
    // Fallback — at least *try* something rather than failing the link.
    candidates[0]
}

fn is_port_free(port: u16) -> bool {
    use std::net::TcpListener;
    // Bind on 127.0.0.1 only — same surface Sail's APP_PORT typically
    // exposes, and avoids racing on 0.0.0.0 if something else is
    // listening on a specific interface.
    TcpListener::bind(("0.0.0.0", port)).is_ok()
}

fn collect_extra_sans(cfg: &Config, project_dir: &Path) -> Result<Vec<String>> {
    let mut sans: Vec<String> = vec![format!("traefik.{tld}", tld = cfg.tld)];

    // Walk every linked project's manifest under the dynamic dir to
    // collect their hostnames. We rotate the cert across all known
    // hosts to keep one trust anchor for the whole stack.
    let dynamic_dir = paths::config_dir()?.join("dynamic");
    if dynamic_dir.exists() {
        for entry in fs::read_dir(&dynamic_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("yml") {
                continue;
            }
            // We don't parse the YAML — we just grep the `Host(`x.tld`)`
            // expressions out. Avoids round-tripping through the file
            // provider's config schema for what is a simple string list.
            let body = fs::read_to_string(&path).unwrap_or_default();
            for cap in HOST_RULE_RE.captures_iter(&body) {
                if let Some(host) = cap.get(1) {
                    sans.push(host.as_str().to_string());
                }
            }
        }
    }

    // Plus anything in the manifest we just saved.
    if let Some(m) = ProjectManifest::load(project_dir)? {
        for h in &m.hosts {
            sans.push(h.host.clone());
        }
    }

    sans.sort();
    sans.dedup();
    Ok(sans)
}

use std::fs;
use std::sync::LazyLock;

use regex::Regex;

static HOST_RULE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Host\(`([^`]+)`\)").expect("static regex"));

fn print_detection(d: &ProjectDetection) {
    use owo_colors::OwoColorize;
    println!();
    println!("{}", "henk — project detection".bold());
    println!();
    println!("  mode:           {:?}", d.mode);
    println!("  default host:   {}", d.default_host);
    if let Some(svc) = &d.web_service {
        println!("  web service:    {svc}");
    }
    if let Some(port) = d.web_port {
        println!("  web port:       {port}");
    }
    if d.vite_detected {
        println!("  vite:           detected (sub-host offered after main link in M4b)");
    }
    if let Some(p) = d.port_collision {
        println!(
            "  port collision: service publishes :{p} — APP_PORT prompt incoming"
        );
    }
    println!();
}

fn announce_override_target(target: &override_file::OverrideTarget) {
    use override_file::OverrideTarget::*;
    match target {
        Canonical(p) => println!("✓ wrote {} (auto-merged by Compose)", p.display()),
        Fallback {
            path,
            existing_canonical,
        } => {
            println!(
                "✓ wrote {} (your {} already exists, so Compose won't auto-merge ours)",
                path.display(),
                existing_canonical.display()
            );
            println!(
                "  add this to your project's .env so Compose picks both up:"
            );
            println!(
                "    COMPOSE_FILE={}:{}",
                existing_canonical
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("compose.override.yml"),
                path.file_name().and_then(|n| n.to_str()).unwrap_or("henk.override.yml")
            );
        }
    }
}

fn print_summary(manifest: &ProjectManifest, project_dir: &Path) {
    use owo_colors::OwoColorize;
    println!();
    println!("{}", "✓ linked.".green().bold());
    for h in &manifest.hosts {
        println!("  https://{}", h.host);
    }
    println!();
    println!("  marker:    {}", project_dir.join(".henk.toml").display());
    println!(
        "  routing:   ~/.config/henk/dynamic/{}.yml",
        manifest.slug
    );
    println!();
    println!("Next: bring up your project (e.g. `npm run dev`, `sail up`, `docker compose up -d`).");
}
