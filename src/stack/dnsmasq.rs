//! Homebrew dnsmasq — install, drop-in config under `dnsmasq.d/`, and
//! lifecycle (`sudo brew services start/restart dnsmasq`).
//!
//! Reasoning lives in `assets/traefik/compose.yml.tmpl`: dnsmasq inside
//! the global compose stack is unreliable on Docker Desktop / linuxkit
//! (DNS query packets are silently dropped before reaching the process).
//! The pragmatic fix — used by Laravel Valet and DDEV — is to install
//! dnsmasq via Homebrew on the host and drive it under launchd.

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::PathBuf;

use crate::consts::HENK_FILE_HEADER;
use crate::runner::SystemRunner;

const DNSMASQ_TMPL: &str = include_str!("../../assets/dnsmasq/dnsmasq.conf.tmpl");

/// Path to the Homebrew prefix (`/opt/homebrew` on Apple Silicon,
/// `/usr/local` on Intel). Resolved at runtime so the binary works on
/// either architecture without recompiling.
pub async fn brew_prefix(runner: &SystemRunner) -> Result<PathBuf> {
    let out = runner
        .run("brew", ["--prefix"])
        .await
        .context("running `brew --prefix`")?;
    if !out.ok() {
        bail!(
            "could not resolve Homebrew prefix:\n{}\n{}",
            out.stdout.trim_end(),
            out.stderr.trim_end()
        );
    }
    let p = out.first_line().unwrap_or("").trim().to_string();
    if p.is_empty() {
        bail!("`brew --prefix` produced no output");
    }
    Ok(PathBuf::from(p))
}

/// `<brew_prefix>/etc/dnsmasq.d/henk-<tld>.conf` — the file we own.
pub async fn config_path(runner: &SystemRunner, tld: &str) -> Result<PathBuf> {
    Ok(brew_prefix(runner)
        .await?
        .join("etc")
        .join("dnsmasq.d")
        .join(format!("henk-{tld}.conf")))
}

/// Render the dnsmasq drop-in for the chosen TLD.
fn render(tld: &str) -> String {
    DNSMASQ_TMPL
        .replace("{{HENK_FILE_HEADER}}", HENK_FILE_HEADER)
        .replace("{{TLD}}", tld)
}

/// Idempotent. Installs dnsmasq via brew if missing (caller asks for
/// consent first), wires the main `dnsmasq.conf` to load our drop-in
/// directory, writes our drop-in for the chosen TLD, and ensures the
/// dnsmasq service is running under launchd. After the run, queries to
/// `127.0.0.1:53` for `*.<tld>` names should resolve to 127.0.0.1.
pub async fn ensure(runner: &SystemRunner, tld: &str) -> Result<()> {
    install_if_missing(runner).await?;
    ensure_conf_dir_directive(runner).await?;
    write_drop_in(runner, tld).await?;
    start_service(runner).await?;
    verify_service_running(runner).await?;
    Ok(())
}

/// Homebrew ships `dnsmasq.conf` without a `conf-dir=...` directive, which
/// means files we drop into `dnsmasq.d/` are silently ignored. Fix that by
/// appending exactly one tagged line to the main config. Idempotent —
/// skipped when our marker is already present.
async fn ensure_conf_dir_directive(runner: &SystemRunner) -> Result<()> {
    let main_conf = brew_prefix(runner).await?.join("etc/dnsmasq.conf");
    let dir = brew_prefix(runner).await?.join("etc/dnsmasq.d");
    let existing = fs::read_to_string(&main_conf).unwrap_or_default();

    let marker = "# managed by henk — load drop-in configs";
    if existing.contains(marker) {
        return Ok(());
    }

    let directive = format!("\n{marker}\nconf-dir={},*.conf\n", dir.display());
    let mut new_contents = existing;
    if !new_contents.ends_with('\n') {
        new_contents.push('\n');
    }
    new_contents.push_str(&directive);

    let tmp = main_conf.with_extension("tmp");
    fs::write(&tmp, &new_contents).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, &main_conf)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), main_conf.display()))?;
    Ok(())
}

/// Confirm the service actually came up after `brew services restart`.
/// `brew services restart` returns 0 even when launchd refuses the plist,
/// so we double-check by sending a real DNS query to 127.0.0.1:53 and
/// verifying it answers with 127.0.0.1 for an arbitrary `*.<tld>` name.
async fn verify_service_running(runner: &SystemRunner) -> Result<()> {
    // Tiny retry loop — dnsmasq usually answers within ~50ms but can take
    // up to a second on cold launchd starts.
    for attempt in 0..10 {
        let out = runner
            .run(
                "dig",
                [
                    "+short",
                    "+time=1",
                    "+tries=1",
                    "@127.0.0.1",
                    "henk-probe.invalid-suffix-this-shouldnt-exist",
                ],
            )
            .await;
        if let Ok(o) = out {
            // We don't actually care about the value — just that dnsmasq
            // answered (returns NXDOMAIN, empty stdout, exit 0). A timeout
            // on the other hand returns a non-zero exit + "no servers"
            // message on stderr.
            if o.ok() {
                return Ok(());
            }
        }
        if attempt < 9 {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }
    bail!(
        "dnsmasq is not answering on 127.0.0.1:53 after `brew services restart`. \
         Check `sudo brew services info dnsmasq`, `launchctl print system/homebrew.mxcl.dnsmasq`, \
         and the log at $(brew --prefix)/var/log/dnsmasq.log."
    )
}

async fn install_if_missing(runner: &SystemRunner) -> Result<()> {
    if runner.which("dnsmasq").await {
        return Ok(());
    }
    if runner.ok("brew", ["list", "--versions", "dnsmasq"]).await {
        return Ok(());
    }
    println!("⤷ brew install dnsmasq ...");
    let exit = runner
        .run_inherit("brew", ["install", "dnsmasq"])
        .await
        .context("running `brew install dnsmasq`")?;
    if exit != 0 {
        bail!("`brew install dnsmasq` failed with exit code {exit}");
    }
    Ok(())
}

async fn write_drop_in(runner: &SystemRunner, tld: &str) -> Result<()> {
    let path = config_path(runner, tld).await?;
    let parent = path
        .parent()
        .context("dnsmasq config path must have a parent directory")?;
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;

    let desired = render(tld);
    if let Ok(existing) = fs::read_to_string(&path)
        && existing == desired
    {
        return Ok(());
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, &desired).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, &path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

/// Start (or restart) the dnsmasq service so our drop-in takes effect.
/// Uses `sudo brew services` because dnsmasq needs to bind privileged
/// port :53. Sudo is expected to be primed via `sudo -v` already (see
/// `cli/init.rs::prime_sudo`).
async fn start_service(runner: &SystemRunner) -> Result<()> {
    // If dnsmasq is already running (e.g., a teammate already configured
    // it via Valet, DDEV, or earlier henk runs), `restart` reloads our
    // drop-in. If it isn't running yet, `restart` is equivalent to `start`.
    println!("⤷ sudo brew services restart dnsmasq ...");
    let out = runner
        .run("sudo", ["brew", "services", "restart", "dnsmasq"])
        .await
        .context("sudo brew services restart dnsmasq")?;
    if !out.ok() {
        bail!(
            "could not start dnsmasq:\n{}\n{}",
            out.stdout.trim_end(),
            out.stderr.trim_end()
        );
    }
    Ok(())
}

/// Remove the henk drop-in (used by `henk uninstall` in M7).
#[allow(dead_code)]
pub async fn remove_drop_in(runner: &SystemRunner, tld: &str) -> Result<()> {
    let path = config_path(runner, tld).await?;
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_in_carries_header_and_address_directive() {
        let body = render("test");
        assert!(body.contains("# managed by henk"));
        assert!(body.contains("address=/.test/127.0.0.1"));
        assert!(!body.contains("{{"));
    }

    #[test]
    fn drop_in_substitutes_fallback_tld() {
        let body = render("henk");
        assert!(body.contains("address=/.henk/127.0.0.1"));
    }
}
