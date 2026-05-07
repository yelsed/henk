//! mkcert wrapper. Installs the local CA into the system keychain (one-time,
//! idempotent) and issues the wildcard certificate used by Traefik.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

use crate::runner::SystemRunner;
use crate::stack::paths;

/// Path the wildcard cert file lives at, e.g. `_wildcard.test.pem`.
pub fn cert_path(tld: &str) -> Result<PathBuf> {
    Ok(paths::traefik_dir()?
        .join("certs")
        .join(format!("_wildcard.{tld}.pem")))
}

/// Path the wildcard key file lives at, e.g. `_wildcard.test-key.pem`.
pub fn key_path(tld: &str) -> Result<PathBuf> {
    Ok(paths::traefik_dir()?
        .join("certs")
        .join(format!("_wildcard.{tld}-key.pem")))
}

/// Run `mkcert -install` so the system keychain trusts the local CA.
/// Idempotent — mkcert is itself a no-op when already installed.
pub async fn ensure_ca_installed(runner: &SystemRunner) -> Result<()> {
    let out = runner
        .run("mkcert", ["-install"])
        .await
        .context("running `mkcert -install`")?;
    if !out.ok() {
        bail!(
            "mkcert -install failed:\n{}\n{}",
            out.stdout.trim_end(),
            out.stderr.trim_end()
        );
    }
    Ok(())
}

/// Generate the wildcard certificate covering `*.<tld>` and `<tld>` itself.
/// Idempotent: if the cert + key already exist on disk, returns without
/// regenerating. Pass `force = true` to always regenerate.
pub async fn ensure_wildcard(
    runner: &SystemRunner,
    tld: &str,
    force: bool,
) -> Result<()> {
    let cert = cert_path(tld)?;
    let key = key_path(tld)?;

    if !force && cert.exists() && key.exists() {
        return Ok(());
    }

    let certs_dir = cert.parent().context("cert path must have a parent")?;
    std::fs::create_dir_all(certs_dir)
        .with_context(|| format!("creating {}", certs_dir.display()))?;

    // mkcert defaults to numbering output files awkwardly when run with
    // multiple SAN hostnames, so we pin -cert-file / -key-file explicitly.
    let cert_str = path_str(&cert)?;
    let key_str = path_str(&key)?;
    let wildcard = format!("*.{tld}");

    let out = runner
        .run(
            "mkcert",
            [
                "-cert-file",
                &cert_str,
                "-key-file",
                &key_str,
                &wildcard,
                tld,
            ],
        )
        .await
        .context("running `mkcert -cert-file ... -key-file ... *.<tld> <tld>`")?;
    if !out.ok() {
        bail!(
            "mkcert wildcard generation failed:\n{}\n{}",
            out.stdout.trim_end(),
            out.stderr.trim_end()
        );
    }
    Ok(())
}

fn path_str(p: &Path) -> Result<String> {
    p.to_str()
        .map(str::to_owned)
        .with_context(|| format!("path {} is not valid UTF-8", p.display()))
}
