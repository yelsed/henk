//! Bring the global Traefik + dnsmasq stack up and down via `docker compose`,
//! plus the broader full-init flow that also handles certs and the resolver
//! file.

use anyhow::{Context, Result, bail};

use crate::config::Config;
use crate::consts::PROXY_NETWORK;
use crate::detect::Status;
use crate::detect::ports::probe_port;
use crate::runner::SystemRunner;
use crate::stack::paths;
use crate::stack::{certs, dnsmasq, resolver, templates};

/// Idempotent. Renders templates from the loaded `Config`, ensures the
/// shared Docker network exists, then runs `docker compose up -d`. Does
/// **not** install certs or write the resolver file — that's `init_full`.
///
/// Refuses to proceed if ports 80 or 443 are bound by anyone else than
/// our own Traefik container. Docker Desktop on macOS silently drops
/// failed port bindings during `docker compose up`, so we must catch
/// this ourselves before the call.
pub async fn up(runner: &SystemRunner, cfg: &Config) -> Result<()> {
    require_docker(runner).await?;
    require_ports_free(runner, cfg).await?;
    templates::render_all(cfg)?;
    ensure_network(runner).await?;
    compose_up(runner).await?;
    print_up_summary(cfg);
    Ok(())
}

/// `henk init` (full mode). Drives the entire first-run setup, in order:
///
/// 1. Render the stack templates from `cfg`.
/// 2. Ensure mkcert's local CA is installed in the system keychain.
/// 3. Generate the wildcard certificate for `*.<tld>`.
/// 4. Write `/etc/resolver/<tld>` with sudo (idempotent — skipped if the
///    correct contents are already in place under our header).
/// 5. Ensure the `henk-proxy` Docker network exists.
/// 6. `docker compose up -d` for the global stack.
///
/// Each step is idempotent; rerunning `henk init` is safe.
pub async fn init_full(runner: &SystemRunner, cfg: &Config) -> Result<()> {
    require_docker(runner).await?;
    require_ports_free(runner, cfg).await?;

    println!("⤷ rendering stack templates ...");
    templates::render_all(cfg)?;

    println!("⤷ ensuring mkcert's local CA is installed (may prompt) ...");
    certs::ensure_ca_installed(runner).await?;

    println!("⤷ issuing wildcard cert for *.{tld} ...", tld = cfg.tld);
    certs::ensure_wildcard(runner, &cfg.tld, false).await?;

    println!("⤷ ensuring Homebrew dnsmasq is installed and running ...");
    dnsmasq::ensure(runner, &cfg.tld).await?;

    println!(
        "⤷ writing /etc/resolver/{tld} (sudo prompt incoming if not cached) ...",
        tld = cfg.tld
    );
    resolver::ensure_written(runner, &cfg.tld).await?;

    println!("⤷ ensuring docker network `{PROXY_NETWORK}` exists ...");
    ensure_network(runner).await?;

    println!("⤷ starting global stack ...");
    compose_up(runner).await?;

    print_up_summary(cfg);
    Ok(())
}

fn print_up_summary(cfg: &Config) {
    println!();
    println!("✓ henk stack is up.");
    println!(
        "  Traefik dashboard: https://traefik.{tld} (or http://localhost:{port})",
        tld = cfg.tld,
        port = cfg.ports.dashboard
    );
}

/// Stop the stack and keep it stopped (`unless-stopped` honours explicit stops).
pub async fn down(runner: &SystemRunner) -> Result<()> {
    require_docker(runner).await?;
    let compose = paths::traefik_compose_path()?;
    if !compose.exists() {
        bail!(
            "henk stack is not configured at {}. Run `henk init` first.",
            compose.display()
        );
    }
    let path = compose.to_str().context("compose path must be UTF-8")?;
    let out = runner
        .run("docker", ["compose", "-f", path, "down"])
        .await
        .context("running `docker compose down`")?;
    if !out.ok() {
        bail!(
            "docker compose down failed:\n{}\n{}",
            out.stdout.trim_end(),
            out.stderr.trim_end()
        );
    }
    println!("✓ henk stack stopped.");
    Ok(())
}

async fn require_docker(runner: &SystemRunner) -> Result<()> {
    let out = runner
        .run("docker", ["info", "--format", "{{.ServerVersion}}"])
        .await;
    match out {
        Ok(o) if o.ok() => Ok(()),
        _ => bail!("Docker is not running. Start Docker Desktop and try again."),
    }
}

async fn require_ports_free(runner: &SystemRunner, cfg: &Config) -> Result<()> {
    let our_traefik_running = is_henk_traefik_running(runner).await;
    if our_traefik_running {
        return Ok(());
    }

    // Check every port we plan to publish on the host. Docker Desktop on
    // macOS will silently drop conflicting bindings during `compose up`
    // rather than erroring, so we must catch this ourselves.
    //
    // dnsmasq's :53 is checked separately in the dnsmasq install path —
    // it lives outside Docker so the failure mode is different.
    let probes = [
        ("host TCP :80", cfg.ports.http, "http"),
        ("host TCP :443", cfg.ports.https, "https"),
        ("loopback :dashboard", cfg.ports.dashboard, "Traefik dashboard"),
    ];
    let mut blockers = Vec::new();
    for (name, port, purpose) in probes {
        let item = probe_port(runner, name, port, purpose).await;
        if item.status == Status::Block {
            blockers.push(item);
        }
    }
    if blockers.is_empty() {
        return Ok(());
    }
    let mut msg = String::from(
        "cannot bind required ports — Traefik would silently lose these bindings under Docker Desktop:\n",
    );
    for b in blockers {
        msg.push_str(&format!("  · {}\n", b.detail));
    }
    msg.push_str("Stop the offending process(es) and re-run.");
    bail!(msg);
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

async fn ensure_network(runner: &SystemRunner) -> Result<()> {
    let exists = runner
        .run(
            "docker",
            [
                "network",
                "ls",
                "--filter",
                &format!("name={PROXY_NETWORK}"),
                "--format",
                "{{.Name}}",
            ],
        )
        .await
        .ok()
        .map(|o| o.stdout.lines().any(|l| l.trim() == PROXY_NETWORK))
        .unwrap_or(false);

    if exists {
        return Ok(());
    }

    let out = runner
        .run("docker", ["network", "create", PROXY_NETWORK])
        .await
        .context("creating docker network `henk-proxy`")?;
    if !out.ok() {
        bail!(
            "could not create network `{PROXY_NETWORK}`:\n{}\n{}",
            out.stdout.trim_end(),
            out.stderr.trim_end()
        );
    }
    Ok(())
}

async fn compose_up(runner: &SystemRunner) -> Result<()> {
    let compose = paths::traefik_compose_path()?;
    let path = compose.to_str().context("compose path must be UTF-8")?;
    let out = runner
        .run(
            "docker",
            ["compose", "-f", path, "up", "-d", "--remove-orphans"],
        )
        .await
        .context("running `docker compose up -d`")?;
    if !out.ok() {
        bail!(
            "docker compose up failed:\n{}\n{}",
            out.stdout.trim_end(),
            out.stderr.trim_end()
        );
    }
    Ok(())
}
