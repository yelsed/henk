//! `henk status` — text snapshot of stack health, linked projects, and
//! certificate state. Read-only.

use anyhow::Result;
use std::path::Path;

use crate::config::Config;
use crate::consts::STACK_VERSION;
use crate::detect::{Status, backend};
use crate::manifest::StateManifest;
use crate::runner::SystemRunner;
use crate::stack::paths;

pub async fn run() -> Result<()> {
    use owo_colors::OwoColorize;

    let runner = SystemRunner::new();
    let cfg = Config::load()?;

    println!();
    println!("{}", "henk — status".bold());
    println!();

    let Some(cfg) = cfg else {
        println!(
            "  {}  henk has not been initialised on this machine yet.",
            "·".bright_black()
        );
        println!("  Run `henk init` to set up the global stack.");
        println!();
        return Ok(());
    };

    println!("  TLD:        .{}", cfg.tld);
    println!("  HTTP/HTTPS: :{} / :{}", cfg.ports.http, cfg.ports.https);
    println!("  Dashboard:  http://localhost:{}", cfg.ports.dashboard);
    println!();

    print_stale_stack_nudge()?;
    print_stack_status(&runner).await;
    print_cert_status(&cfg).await;
    print_linked_projects(&runner).await?;

    Ok(())
}

/// The binary can be replaced without henk being the one to do it (Homebrew, the
/// installer script, `cargo install`), and a newer binary ships newer stack
/// templates than the running proxy booted with. `henk update` re-renders on its
/// own; this is what catches every other route in.
fn print_stale_stack_nudge() -> Result<()> {
    use owo_colors::OwoColorize;

    let Some(state) = StateManifest::load()? else {
        return Ok(());
    };
    if state.stack_version >= STACK_VERSION {
        return Ok(());
    }

    println!(
        "  {}  The running stack is v{} but this henk ships v{STACK_VERSION}.",
        "!".yellow(),
        state.stack_version
    );
    println!("     Run {} to pick up the new routing.", "henk up".bold());
    println!();
    Ok(())
}

async fn print_stack_status(runner: &SystemRunner) {
    use owo_colors::OwoColorize;

    let traefik = container_running(runner, "henk-traefik").await;
    let traefik_str = if traefik {
        "running".green().to_string()
    } else {
        "stopped".red().to_string()
    };
    println!("  Traefik:    {traefik_str}");

    // Homebrew dnsmasq runs under launchd, not docker. Probe via `dig`
    // to see if :53 actually answers — `brew services` lies (see M3.5).
    let dnsmasq = dnsmasq_answering(runner).await;
    let dnsmasq_str = if dnsmasq {
        "answering on 127.0.0.1:53".green().to_string()
    } else {
        "not answering on :53".red().to_string()
    };
    println!("  dnsmasq:    {dnsmasq_str}");
    println!();
}

async fn print_cert_status(cfg: &Config) {
    use owo_colors::OwoColorize;

    let cert_path = paths::traefik_dir()
        .ok()
        .map(|p| p.join("certs").join(format!("_wildcard.{}.pem", cfg.tld)));
    match cert_path.as_deref() {
        Some(p) if p.exists() => {
            let sans = read_cert_sans(p).unwrap_or_default();
            let expires = read_cert_expiry(p).unwrap_or_else(|| "unknown".into());
            println!("  Cert:       {} SANs, expires {}", sans.len(), expires);
            if !sans.is_empty() {
                let preview: Vec<_> = sans.iter().take(6).collect();
                let suffix = if sans.len() > 6 { ", …" } else { "" };
                println!(
                    "              {}{suffix}",
                    preview
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
        _ => {
            println!("  Cert:       {}", "not issued".red());
        }
    }
    println!();
}

async fn print_linked_projects(runner: &SystemRunner) -> Result<()> {
    use owo_colors::OwoColorize;

    let linked = backend::linked_hosts()?;
    if linked.is_empty() {
        println!("  Linked projects: {}", "none".bright_black());
        println!();
        return Ok(());
    }

    // Host-mode backends get a reachability verdict; Docker-mode ones can't be
    // probed from here, so they simply print without a glyph.
    let verdicts = backend::probe_all(runner).await?;

    println!("  {}", "Linked projects:".bold());
    let mut current_slug = "";
    for host in &linked {
        if host.slug != current_slug {
            println!("    · {}", host.slug);
            current_slug = &host.slug;
        }
        match verdicts.iter().find(|v| v.host == host.host) {
            Some(v) if v.status == Status::Ok => {
                println!("        {}  https://{}", v.status.glyph(), host.host)
            }
            Some(v) => println!(
                "        {}  https://{}  — {}",
                v.status.glyph(),
                host.host,
                v.detail
            ),
            None => println!("           https://{}", host.host),
        }
    }
    println!();
    Ok(())
}

async fn container_running(runner: &SystemRunner, name: &str) -> bool {
    let out = runner
        .run(
            "docker",
            [
                "ps",
                "--filter",
                &format!("name={name}"),
                "--format",
                "{{.Names}}",
            ],
        )
        .await;
    matches!(out, Ok(o) if o.ok() && o.stdout.lines().any(|l| l.trim() == name))
}

async fn dnsmasq_answering(runner: &SystemRunner) -> bool {
    // `dig` against 127.0.0.1 hits the host dnsmasq directly. We probe a
    // bogus name under the henk TLD; we only care that *something*
    // responds, not the answer.
    let cfg = Config::load().ok().flatten();
    let tld = cfg.as_ref().map(|c| c.tld.as_str()).unwrap_or("test");
    let probe = format!("henk-status-probe.{tld}");
    let out = runner
        .run(
            "dig",
            ["+short", "+time=1", "+tries=1", "@127.0.0.1", &probe],
        )
        .await;
    match out {
        Ok(o) => o.ok(),
        Err(_) => false,
    }
}

/// Subject Alternative Names from a PEM-encoded x509 cert. Shells out
/// to `openssl` — adding an x509 parser as a dep just for status would
/// be overkill, and `openssl` is on every macOS box.
fn read_cert_sans(path: &Path) -> Option<Vec<String>> {
    let out = std::process::Command::new("openssl")
        .args(["x509", "-in"])
        .arg(path)
        .args(["-noout", "-text"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // Find `Subject Alternative Name:` then the line after it.
    let mut iter = text.lines();
    while let Some(line) = iter.next() {
        if line
            .trim_start()
            .starts_with("X509v3 Subject Alternative Name")
        {
            let next = iter.next()?.trim();
            return Some(
                next.split(',')
                    .filter_map(|s| s.trim().strip_prefix("DNS:").map(str::to_string))
                    .collect(),
            );
        }
    }
    None
}

fn read_cert_expiry(path: &Path) -> Option<String> {
    let out = std::process::Command::new("openssl")
        .args(["x509", "-in"])
        .arg(path)
        .args(["-noout", "-enddate"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text.trim().strip_prefix("notAfter=").map(|s| s.to_string())
}
