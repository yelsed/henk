//! `henk status` — text snapshot of stack health, linked projects, and
//! certificate state. Read-only.

use anyhow::{Context, Result};
use std::path::Path;

use crate::config::Config;
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

    print_stack_status(&runner).await;
    print_cert_status(&cfg).await;
    print_linked_projects(&cfg)?;

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

fn print_linked_projects(_cfg: &Config) -> Result<()> {
    use owo_colors::OwoColorize;

    let dyn_dir = paths::dynamic_projects_dir()?;
    if !dyn_dir.exists() {
        println!("  Linked projects: {}", "none".bright_black());
        println!();
        return Ok(());
    }

    let mut entries: Vec<(String, Vec<String>)> = Vec::new();
    for entry in
        std::fs::read_dir(&dyn_dir).with_context(|| format!("reading {}", dyn_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("yml") {
            continue;
        }
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        // `_henk.yml` is the dashboard / TLS config — not a linked project.
        if stem.starts_with('_') {
            continue;
        }
        let body = std::fs::read_to_string(&path).unwrap_or_default();
        entries.push((stem, extract_hosts_from_yaml(&body)));
    }

    if entries.is_empty() {
        println!("  Linked projects: {}", "none".bright_black());
        println!();
        return Ok(());
    }

    println!("  {}", "Linked projects:".bold());
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    for (slug, hosts) in &entries {
        println!("    · {slug}");
        for h in hosts {
            println!("        https://{h}");
        }
    }
    println!();
    Ok(())
}

fn extract_hosts_from_yaml(body: &str) -> Vec<String> {
    use std::sync::LazyLock;
    static RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"Host\(`([^`]+)`\)").expect("static regex"));
    RE.captures_iter(body)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_hosts_from_yaml_pulls_every_host_rule() {
        let body = r#"
http:
  routers:
    a:
      rule: "Host(`one.test`)"
    b:
      rule: "Host(`two.test`)"
        "#;
        assert_eq!(
            extract_hosts_from_yaml(body),
            vec!["one.test".to_string(), "two.test".to_string()]
        );
    }

    #[test]
    fn extract_hosts_returns_empty_on_no_match() {
        assert!(extract_hosts_from_yaml("services: {}").is_empty());
    }
}
