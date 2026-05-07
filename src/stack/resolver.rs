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

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::PathBuf;

use crate::consts::HENK_FILE_HEADER;
use crate::runner::SystemRunner;

pub fn resolver_path(tld: &str) -> PathBuf {
    PathBuf::from(format!("/etc/resolver/{tld}"))
}

/// Render the resolver-file body for our chosen TLD.
fn render() -> String {
    format!("{HENK_FILE_HEADER}\nnameserver 127.0.0.1\n")
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
pub async fn ensure_written(runner: &SystemRunner, tld: &str) -> Result<()> {
    match status(tld) {
        ResolverStatus::Ours => {
            let desired = render();
            let current = fs::read_to_string(resolver_path(tld)).unwrap_or_default();
            if current == desired {
                return Ok(());
            }
            install_with_sudo(runner, tld, &desired).await
        }
        ResolverStatus::Missing => install_with_sudo(runner, tld, &render()).await,
        ResolverStatus::Foreign => bail!(
            "/etc/resolver/{tld} exists but is not managed by henk.\n\
             Refusing to overwrite. Either remove that file manually or pick a different TLD."
        ),
    }
}

/// Remove `/etc/resolver/<tld>` if (and only if) it carries our header.
#[allow(dead_code)] // wired up by `henk uninstall` in M7.
pub async fn ensure_removed(runner: &SystemRunner, tld: &str) -> Result<()> {
    match status(tld) {
        ResolverStatus::Missing => Ok(()),
        ResolverStatus::Foreign => bail!(
            "/etc/resolver/{tld} exists but is not managed by henk; not removing"
        ),
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
            Ok(())
        }
    }
}

async fn install_with_sudo(
    runner: &SystemRunner,
    tld: &str,
    contents: &str,
) -> Result<()> {
    let target = resolver_path(tld);
    let target_str = target.to_str().context("resolver path must be UTF-8")?;

    // Write payload to a temp file owned by us, then have sudo install it
    // atomically into /etc/resolver/<tld> with the right perms.
    let tmp_dir = std::env::temp_dir();
    let tmp = tmp_dir.join(format!("henk-resolver-{tld}-{}", std::process::id()));
    fs::write(&tmp, contents).with_context(|| format!("writing temp {}", tmp.display()))?;
    let tmp_str = tmp.to_str().context("tmp path must be UTF-8")?;

    // `install` creates intermediate directories if needed (we add `-D`).
    // Mode 0644 matches Valet's resolver-file permissions.
    let out = runner
        .run(
            "sudo",
            [
                "install",
                "-d",
                "-m",
                "0755",
                "/etc/resolver",
            ],
        )
        .await
        .context("sudo install -d /etc/resolver")?;
    if !out.ok() {
        let _ = fs::remove_file(&tmp);
        bail!(
            "could not ensure /etc/resolver exists:\n{}\n{}",
            out.stdout.trim_end(),
            out.stderr.trim_end()
        );
    }

    let out = runner
        .run(
            "sudo",
            ["install", "-m", "0644", tmp_str, target_str],
        )
        .await
        .context("sudo install /tmp/<...> /etc/resolver/<tld>")?;
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
