//! Bring the global Traefik stack up and down via `docker compose`.

use anyhow::{Context, Result, bail};

use crate::consts::{HTTP_PORT, HTTPS_PORT, PROXY_NETWORK};
use crate::detect::Status;
use crate::detect::ports::probe_port;
use crate::runner::SystemRunner;
use crate::stack::paths;
use crate::stack::templates;

/// Idempotent. Renders templates if needed, ensures the shared Docker
/// network exists, then runs `docker compose up -d`.
///
/// Refuses to proceed if ports 80 or 443 are bound by anyone else than
/// our own Traefik container. Docker Desktop on macOS silently drops
/// failed port bindings during `docker compose up`, so we must catch
/// this ourselves before the call.
pub async fn up(runner: &SystemRunner) -> Result<()> {
    require_docker(runner).await?;
    require_ports_free(runner).await?;
    templates::render_all()?;
    ensure_network(runner).await?;
    compose_up(runner).await?;
    println!("✓ henk stack is up.");
    println!(
        "  Traefik dashboard: http://traefik.localhost (or http://localhost:{})",
        crate::consts::DASHBOARD_PORT
    );
    Ok(())
}

async fn require_ports_free(runner: &SystemRunner) -> Result<()> {
    // If our own henk-traefik container already holds these ports, that's
    // fine — `compose up` is idempotent. We only refuse for foreign holders.
    let our_traefik_running = is_henk_traefik_running(runner).await;
    if our_traefik_running {
        return Ok(());
    }

    let http = probe_port(runner, "host TCP :80", HTTP_PORT, "http").await;
    let https = probe_port(runner, "host TCP :443", HTTPS_PORT, "https").await;

    let mut blockers = Vec::new();
    if http.status == Status::Block {
        blockers.push(http);
    }
    if https.status == Status::Block {
        blockers.push(https);
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
    msg.push_str("Stop the offending process(es) and re-run `henk up`.");
    bail!(msg);
}

async fn is_henk_traefik_running(runner: &SystemRunner) -> bool {
    let out = runner
        .run(
            "docker",
            ["ps", "--filter", "name=henk-traefik", "--format", "{{.Names}}"],
        )
        .await;
    matches!(out, Ok(o) if o.ok() && o.stdout.lines().any(|l| l.trim() == "henk-traefik"))
}

/// Stop the stack and keep it stopped (`unless-stopped` honours explicit stops).
pub async fn down(runner: &SystemRunner) -> Result<()> {
    require_docker(runner).await?;
    let compose = paths::traefik_compose_path()?;
    if !compose.exists() {
        bail!(
            "henk stack is not configured at {}. Run `henk up` (or `henk init`) first.",
            compose.display()
        );
    }
    let out = runner
        .run("docker", ["compose", "-f", compose.to_str().unwrap(), "down"])
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
    let out = runner.run("docker", ["info", "--format", "{{.ServerVersion}}"]).await;
    match out {
        Ok(o) if o.ok() => Ok(()),
        _ => bail!(
            "Docker is not running. Start Docker Desktop and try again."
        ),
    }
}

async fn ensure_network(runner: &SystemRunner) -> Result<()> {
    let exists = runner
        .run(
            "docker",
            ["network", "ls", "--filter", &format!("name={PROXY_NETWORK}"), "--format", "{{.Name}}"],
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
        .run("docker", ["compose", "-f", path, "up", "-d", "--remove-orphans"])
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
