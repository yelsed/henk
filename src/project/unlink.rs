//! `henk unlink` — symmetric reverse of `henk link`.
//!
//! Two modes:
//!   - `henk unlink` (no host) → de-register the entire project: drop
//!     the file-provider entry, the compose override (only if we own
//!     it), and `.henk.toml`.
//!   - `henk unlink <host>` → drop a single host. If it was the last
//!     one, fall through to the project-wide path.
//!
//! Cert SANs are regenerated to drop the just-removed host(s), and the
//! Traefik container is restarted so the in-memory cert is replaced.
//!
//! Symmetry guarantee: every file we delete here is one we wrote in
//! `link::run`. Foreign overrides, foreign `.henk.toml`s — refused.
//! `.env` lines we appended are NOT auto-removed (that crosses the
//! "never edit existing lines" line); the printed summary tells the
//! user which APP_PORT to drop manually.

use anyhow::{Context, Result, bail};
use std::path::Path;

use crate::config::Config;
use crate::consts::HENK_FILE_HEADER;
use crate::project::manifest::ProjectManifest;
use crate::project::{file_provider, override_file};
use crate::runner::SystemRunner;
use crate::stack::{certs, paths};

/// Run unlink against `project_dir`. `host` filters to a single host;
/// `None` removes the whole project.
pub async fn run(
    runner: &SystemRunner,
    project_dir: &Path,
    host: Option<String>,
) -> Result<()> {
    let cfg = Config::load()?
        .context("henk has not been initialised yet — nothing to unlink against")?;

    let mut manifest = ProjectManifest::load(project_dir)?
        .context("this directory isn't linked — no `.henk.toml` to unlink")?;

    let removed_hosts: Vec<String> = match host {
        Some(h) => {
            let before = manifest.hosts.len();
            manifest.hosts.retain(|entry| entry.host != h);
            if manifest.hosts.len() == before {
                bail!(
                    "host `{h}` is not registered for this project. \
                     Run `henk status` to see linked hosts."
                );
            }
            vec![h]
        }
        None => {
            let drained: Vec<String> =
                manifest.hosts.iter().map(|h| h.host.clone()).collect();
            manifest.hosts.clear();
            drained
        }
    };

    let project_now_empty = manifest.hosts.is_empty();

    if project_now_empty {
        // Full project teardown. Order: file-provider entry first
        // (Traefik watches the dir and reloads instantly), then the
        // override file, then the marker.
        file_provider::remove(&manifest.slug)?;
        override_file::remove(project_dir)?;
        remove_manifest_file(project_dir)?;
    } else {
        // Just dropped a sub-host (e.g. `vite.<app>.test`). Re-render
        // the file-provider with the remaining hosts and persist.
        manifest.save(project_dir)?;
        file_provider::write(&manifest)?;
    }

    // Rotate the wildcard cert so the removed hostnames stop being
    // covered (cert was union of every linked SAN), then restart
    // Traefik so the in-memory cert is replaced.
    let extra_sans = collect_remaining_sans(&cfg)?;
    certs::ensure_wildcard(runner, &cfg.tld, &extra_sans, true).await?;
    if is_henk_traefik_running(runner).await {
        let _ = runner.run("docker", ["restart", "henk-traefik"]).await;
    }

    print_summary(&manifest.slug, &removed_hosts, project_now_empty, project_dir);
    Ok(())
}

/// Delete `<dir>/.henk.toml` only when we authored it (header check).
fn remove_manifest_file(project_dir: &Path) -> Result<()> {
    let path = ProjectManifest::path_in(project_dir);
    if !path.exists() {
        return Ok(());
    }
    let body = std::fs::read_to_string(&path).unwrap_or_default();
    if !body.contains(HENK_FILE_HEADER) {
        bail!(
            "{} is not managed by henk — refusing to delete",
            path.display()
        );
    }
    std::fs::remove_file(&path)
        .with_context(|| format!("removing {}", path.display()))?;
    Ok(())
}

/// Walk every remaining file-provider YAML and collect the hostnames
/// they route. The cert needs to cover every other linked project, plus
/// `traefik.<tld>` for the dashboard.
fn collect_remaining_sans(cfg: &Config) -> Result<Vec<String>> {
    use std::sync::LazyLock;
    static HOST_RULE_RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"Host\(`([^`]+)`\)").expect("static regex"));

    let mut sans: Vec<String> = vec![format!("traefik.{}", cfg.tld)];
    let dir = paths::dynamic_projects_dir()?;
    if dir.exists() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("yml") {
                continue;
            }
            let body = std::fs::read_to_string(&path).unwrap_or_default();
            for cap in HOST_RULE_RE.captures_iter(&body) {
                if let Some(host) = cap.get(1) {
                    sans.push(host.as_str().to_string());
                }
            }
        }
    }
    sans.sort();
    sans.dedup();
    Ok(sans)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::HENK_FILE_HEADER;
    use tempfile::TempDir;

    #[test]
    fn remove_manifest_file_only_drops_henk_authored() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".henk.toml");
        std::fs::write(&path, "# someone else's marker file\nslug = \"x\"\n").unwrap();
        let err = remove_manifest_file(dir.path()).unwrap_err();
        assert!(format!("{err}").contains("not managed by henk"));
        assert!(path.exists(), "foreign .henk.toml must not be deleted");
    }

    #[test]
    fn remove_manifest_file_drops_when_header_present() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".henk.toml");
        std::fs::write(
            &path,
            format!("{HENK_FILE_HEADER}\nslug = \"x\"\n"),
        )
        .unwrap();
        remove_manifest_file(dir.path()).unwrap();
        assert!(!path.exists(), "henk-authored .henk.toml must be deleted");
    }

    #[test]
    fn remove_manifest_file_no_op_when_missing() {
        let dir = TempDir::new().unwrap();
        // No file present — must not error.
        remove_manifest_file(dir.path()).unwrap();
    }
}

fn print_summary(slug: &str, removed: &[String], project_gone: bool, project_dir: &Path) {
    use owo_colors::OwoColorize;
    println!();
    println!("{}", "✓ unlinked.".green().bold());
    for h in removed {
        println!("  - https://{h}");
    }
    println!();
    if project_gone {
        println!("  removed `.henk.toml`, the compose override (if we owned it),");
        println!("  and ~/.config/henk/dynamic/{slug}.yml.");
        println!();
        println!(
            "  If your `.env` carries an `APP_PORT=...` line we appended, drop it"
        );
        println!(
            "  manually — henk doesn't edit existing `.env` lines on unlink ({}).",
            project_dir.display()
        );
    } else {
        println!(
            "  remaining hosts re-rendered to ~/.config/henk/dynamic/{slug}.yml."
        );
    }
}
