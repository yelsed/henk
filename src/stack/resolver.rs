//! Manage `/etc/resolver/<tld>` so macOS resolves `*.<tld>` via the
//! Homebrew dnsmasq listening on 127.0.0.1:53.
//!
//! macOS reads files under `/etc/resolver/` (see `man 5 resolver`) and
//! per-TLD overrides the system DNS for matching names. The resolver
//! file we write is the minimal possible:
//!
//! ```text
//! # managed by henk
//! nameserver 127.0.0.1
//! ```
//!
//! No `port` directive — `dnsmasq` listens on the default :53. Earlier
//! versions of henk targeted an in-stack dnsmasq on :35353; that path
//! was abandoned in M3.5 because Docker Desktop on macOS silently drops
//! DNS packets to in-container dnsmasq.
//!
//! Linux has no `/etc/resolver/`. There the same role is played by a
//! systemd-resolved drop-in that routes `~<tld>` to the in-stack dnsmasq on
//! `127.0.0.1:<dnsmasq port>` (native Docker delivers those packets fine):
//!
//! ```text
//! # managed by henk
//! [Resolve]
//! DNS=127.0.0.1:35353
//! Domains=~test
//! ```

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::PathBuf;

use crate::consts::HENK_FILE_HEADER;
use crate::runner::SystemRunner;

/// Directory systemd-resolved reads drop-ins from.
const RESOLVED_DROPIN_DIR: &str = "/etc/systemd/resolved.conf.d";

pub fn resolver_path(tld: &str) -> PathBuf {
    path_for(tld, cfg!(target_os = "linux"))
}

fn path_for(tld: &str, linux: bool) -> PathBuf {
    if linux {
        PathBuf::from(format!("{RESOLVED_DROPIN_DIR}/henk-{tld}.conf"))
    } else {
        PathBuf::from(format!("/etc/resolver/{tld}"))
    }
}

/// Render the resolver-file body for our chosen TLD.
fn render(tld: &str, dnsmasq_port: u16) -> String {
    render_for(tld, dnsmasq_port, cfg!(target_os = "linux"))
}

fn render_for(tld: &str, dnsmasq_port: u16, linux: bool) -> String {
    if linux {
        format!("{HENK_FILE_HEADER}\n[Resolve]\nDNS=127.0.0.1:{dnsmasq_port}\nDomains=~{tld}\n")
    } else {
        format!("{HENK_FILE_HEADER}\nnameserver 127.0.0.1\n")
    }
}

/// Status of the on-disk resolver file:
/// - `Missing` — file doesn't exist; we need to create it.
/// - `Ours` — exists and carries our header.
/// - `Foreign` — exists without our header (Valet, Herd, manual).
pub enum ResolverStatus {
    Missing,
    Ours,
    Foreign,
}

pub fn status(tld: &str) -> ResolverStatus {
    let path = resolver_path(tld);
    if !path.exists() {
        return ResolverStatus::Missing;
    }
    match fs::read_to_string(&path) {
        Ok(c) if c.contains(HENK_FILE_HEADER) => ResolverStatus::Ours,
        _ => ResolverStatus::Foreign,
    }
}

/// Idempotent. Writes `/etc/resolver/<tld>` with our header. Refuses if a
/// foreign resolver file already exists for this TLD (caller should pick a
/// different TLD or remove the file manually).
///
/// Uses sudo. Caller is expected to have primed credentials via `sudo -v`
/// (see the `init` consent flow). The file is written via `sudo install`
/// for atomicity.
pub async fn ensure_written(runner: &SystemRunner, tld: &str, dnsmasq_port: u16) -> Result<()> {
    let desired = render(tld, dnsmasq_port);
    match status(tld) {
        ResolverStatus::Ours => {
            let current = fs::read_to_string(resolver_path(tld)).unwrap_or_default();
            if current == desired {
                return Ok(());
            }
        }
        ResolverStatus::Missing => {}
        ResolverStatus::Foreign => bail!(
            "{} exists but is not managed by henk.\n\
             Refusing to overwrite. Either remove that file manually or pick a different TLD.",
            resolver_path(tld).display()
        ),
    }
    install_with_sudo(runner, tld, &desired).await?;
    restart_resolved(runner).await
}

/// systemd-resolved only reads its drop-ins at start. No-op on macOS, which
/// picks up `/etc/resolver/` changes by itself.
pub async fn restart_resolved(runner: &SystemRunner) -> Result<()> {
    if !cfg!(target_os = "linux") {
        return Ok(());
    }
    let out = runner
        .run("sudo", ["systemctl", "restart", "systemd-resolved"])
        .await
        .context("sudo systemctl restart systemd-resolved")?;
    if !out.ok() {
        bail!(
            "could not restart systemd-resolved:\n{}\n{}",
            out.stdout.trim_end(),
            out.stderr.trim_end()
        );
    }
    Ok(())
}

/// Remove `/etc/resolver/<tld>` if (and only if) it carries our header.
#[allow(dead_code)] // `henk uninstall` removes by the path state.json recorded.
pub async fn ensure_removed(runner: &SystemRunner, tld: &str) -> Result<()> {
    match status(tld) {
        ResolverStatus::Missing => Ok(()),
        ResolverStatus::Foreign => {
            bail!("/etc/resolver/{tld} exists but is not managed by henk; not removing")
        }
        ResolverStatus::Ours => {
            let path = resolver_path(tld);
            let path_str = path.to_str().context("resolver path must be UTF-8")?;
            let out = runner
                .run("sudo", ["rm", "-f", path_str])
                .await
                .context("sudo rm /etc/resolver/<tld>")?;
            if !out.ok() {
                bail!(
                    "could not remove {}:\n{}\n{}",
                    path_str,
                    out.stdout.trim_end(),
                    out.stderr.trim_end()
                );
            }
            restart_resolved(runner).await
        }
    }
}

async fn install_with_sudo(runner: &SystemRunner, tld: &str, contents: &str) -> Result<()> {
    let target = resolver_path(tld);
    let target_str = target.to_str().context("resolver path must be UTF-8")?;

    let target_dir = target
        .parent()
        .context("resolver path must have a parent directory")?;
    let target_dir_str = target_dir.to_str().context("resolver path must be UTF-8")?;

    // Write payload to a temp file owned by us, then have sudo install it
    // atomically into place with the right perms.
    let tmp_dir = std::env::temp_dir();
    let tmp = tmp_dir.join(format!("henk-resolver-{tld}-{}", std::process::id()));
    fs::write(&tmp, contents).with_context(|| format!("writing temp {}", tmp.display()))?;
    let tmp_str = tmp.to_str().context("tmp path must be UTF-8")?;

    // `install` creates intermediate directories if needed (we add `-D`).
    // Mode 0644 matches Valet's resolver-file permissions.
    let out = runner
        .run("sudo", ["install", "-d", "-m", "0755", target_dir_str])
        .await
        .with_context(|| format!("sudo install -d {target_dir_str}"))?;
    if !out.ok() {
        let _ = fs::remove_file(&tmp);
        bail!(
            "could not ensure {target_dir_str} exists:\n{}\n{}",
            out.stdout.trim_end(),
            out.stderr.trim_end()
        );
    }

    let out = runner
        .run("sudo", ["install", "-m", "0644", tmp_str, target_str])
        .await
        .with_context(|| format!("sudo install /tmp/<...> {target_str}"))?;
    let _ = fs::remove_file(&tmp);
    if !out.ok() {
        bail!(
            "could not write {}:\n{}\n{}",
            target_str,
            out.stdout.trim_end(),
            out.stderr.trim_end()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_resolver_file_points_at_host_dnsmasq() {
        assert_eq!(path_for("test", false), PathBuf::from("/etc/resolver/test"));
        let body = render_for("test", 35353, false);
        assert!(body.starts_with(HENK_FILE_HEADER));
        assert!(body.ends_with("\nnameserver 127.0.0.1\n"));
    }

    #[test]
    fn linux_dropin_routes_only_the_tld_to_the_stack_dnsmasq() {
        assert_eq!(
            path_for("henk", true),
            PathBuf::from("/etc/systemd/resolved.conf.d/henk-henk.conf")
        );
        let body = render_for("henk", 35353, true);
        assert!(body.starts_with(HENK_FILE_HEADER));
        assert!(body.contains("\n[Resolve]\n"));
        assert!(body.contains("\nDNS=127.0.0.1:35353\n"));
        assert!(
            body.contains("\nDomains=~henk\n"),
            "route-only, or it becomes a search domain"
        );
    }
}
