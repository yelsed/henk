//! Backend reachability probes for linked projects.
//!
//! Traefik runs in Docker and reaches host-mode dev servers over IPv4 via
//! `host.docker.internal`. Frameworks default to loopback — often `[::1]`
//! only — so the port looks alive from the terminal but is invisible to the
//! proxy. The failure surfaces as a raw `502`, or worse, a `426 Upgrade
//! Required` when the only IPv4 listener on the port is the framework's HMR
//! WebSocket server. Neither tells the user what to do.
//!
//! These probes name the cause. Read-only.

use anyhow::{Context, Result};
use serde_yaml_ng::Value;

use crate::detect::Status;
use crate::runner::SystemRunner;
use crate::stack::paths;

/// One linked host, as recorded in `~/.config/henk/dynamic/<slug>.yml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedHost {
    pub slug: String,
    pub host: String,
    /// Backend URL Traefik forwards to, e.g. `http://host.docker.internal:3001`.
    pub url: String,
}

impl LinkedHost {
    /// The port to probe, but only for host-mode backends. Docker-mode
    /// backends resolve on the `henk-proxy` network (`http://web:80`) and
    /// aren't reachable from the host at all, so probing them from here
    /// would report a false failure.
    fn host_mode_port(&self) -> Option<u16> {
        let rest = self.url.strip_prefix("http://")?;
        let (authority, _) = rest.split_once('/').unwrap_or((rest, ""));
        let (hostname, port) = authority.rsplit_once(':')?;
        if hostname != "host.docker.internal" {
            return None;
        }
        port.parse().ok()
    }
}

/// The verdict for one linked host.
#[derive(Debug, Clone)]
pub struct BackendItem {
    pub slug: String,
    pub host: String,
    pub status: Status,
    pub detail: String,
}

/// What the host's own IPv4 loopback said when we knocked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortResponse {
    /// Nothing accepted the connection.
    Refused,
    /// The port answered with this HTTP status.
    Code(u16),
}

/// Raw observations for one port, before interpretation.
#[derive(Debug, Clone, Copy)]
pub struct Observation {
    pub plain: PortResponse,
    /// Status when the request carries the real `Host:` header. Only
    /// collected when `plain` looked healthy.
    pub with_host: Option<u16>,
    /// Something is listening, but only on the IPv6 loopback.
    pub ipv6_only_listener: bool,
}

/// Probe every linked host and report what's wrong. Hosts whose backend
/// lives inside Docker are skipped — see [`LinkedHost::host_mode_port`].
pub async fn probe_all(runner: &SystemRunner) -> Result<Vec<BackendItem>> {
    let linked = linked_hosts()?;
    let mut items = Vec::new();
    for host in linked {
        let Some(port) = host.host_mode_port() else {
            continue;
        };
        let observation = observe(runner, port, &host.host).await;
        let (status, detail) = classify(port, &host.host, observation);
        items.push(BackendItem {
            slug: host.slug,
            host: host.host,
            status,
            detail,
        });
    }
    Ok(items)
}

/// Knock on the port the way Traefik does: over IPv4, from outside the
/// framework's loopback assumptions.
async fn observe(runner: &SystemRunner, port: u16, host: &str) -> Observation {
    let plain = http_probe(runner, port, None).await;
    let with_host = match plain {
        PortResponse::Code(code) if is_alive(code) => {
            match http_probe(runner, port, Some(host)).await {
                PortResponse::Code(c) => Some(c),
                PortResponse::Refused => None,
            }
        }
        _ => None,
    };
    let ipv6_only_listener =
        matches!(plain, PortResponse::Refused) && ipv6_only(runner, port).await;
    Observation {
        plain,
        with_host,
        ipv6_only_listener,
    }
}

/// Turn observations into a verdict a user can act on.
pub fn classify(port: u16, host: &str, obs: Observation) -> (Status, String) {
    match obs.plain {
        PortResponse::Refused if obs.ipv6_only_listener => (
            Status::Block,
            format!(
                "port {port} is bound to the IPv6 loopback only — Docker reaches the host over \
                 IPv4, so Traefik can't see it. Restart the dev server on 0.0.0.0 \
                 (`nuxt dev --host`, `vite --host`, `next dev -H 0.0.0.0`)"
            ),
        ),
        PortResponse::Refused => (
            Status::Warn,
            format!("nothing listening on port {port} — is the dev server running?"),
        ),
        // A WebSocket-only server (a framework's HMR socket) answers a plain
        // GET with 426. It bound the IPv4 wildcard while the app itself stayed
        // on the IPv6 loopback, so this is the misbinding wearing a disguise.
        PortResponse::Code(426) => (
            Status::Block,
            format!(
                "port {port} only answers WebSocket upgrades — that's the HMR socket, not the \
                 app. The app is bound to the IPv6 loopback; restart the dev server on 0.0.0.0"
            ),
        ),
        PortResponse::Code(_) if obs.with_host == Some(403) => (
            Status::Block,
            format!(
                "the dev server rejects the hostname `{host}` — add it to the framework's \
                 allowed hosts (Vite: `server.allowedHosts`, Nuxt: `vite.server.allowedHosts`)"
            ),
        ),
        PortResponse::Code(code) if is_alive(code) => {
            (Status::Ok, format!("reachable on port {port} ({code})"))
        }
        PortResponse::Code(code) => (
            Status::Warn,
            format!(
                "responds {code} on / — Traefik's health check counts that as down. Point it at \
                 a path that returns 2xx/3xx with `health_path` in .henk.toml"
            ),
        ),
    }
}

/// Traefik's health check treats 2xx/3xx as alive; everything else is down.
fn is_alive(code: u16) -> bool {
    (200..400).contains(&code)
}

/// One HTTP GET over IPv4, optionally carrying the real `Host:` header.
/// `curl` is on every macOS box and the repo already shells out for
/// everything else — an HTTP client dep would earn nothing here.
async fn http_probe(runner: &SystemRunner, port: u16, host_header: Option<&str>) -> PortResponse {
    let url = format!("http://127.0.0.1:{port}/");
    let mut args = vec![
        "-s".to_string(),
        "-o".to_string(),
        "/dev/null".to_string(),
        "-w".to_string(),
        "%{http_code}".to_string(),
        "--max-time".to_string(),
        "2".to_string(),
    ];
    if let Some(h) = host_header {
        args.push("-H".to_string());
        args.push(format!("Host: {h}"));
    }
    args.push(url);

    match runner.run("curl", args).await {
        Ok(out) if out.ok() => out
            .first_line()
            .and_then(|l| l.parse::<u16>().ok())
            .filter(|c| *c != 0)
            .map(PortResponse::Code)
            .unwrap_or(PortResponse::Refused),
        _ => PortResponse::Refused,
    }
}

/// Is the only listener on this port bound to the IPv6 loopback?
async fn ipv6_only(runner: &SystemRunner, port: u16) -> bool {
    let out = runner
        .run("lsof", ["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN"])
        .await;
    match out {
        Ok(o) if o.ok() => listeners_are_ipv6_only(&o.stdout),
        _ => false,
    }
}

/// `lsof -nP -iTCP:3001 -sTCP:LISTEN` prints one line per listening socket,
/// ending in `<bind address> (LISTEN)` — where the bind is `[::1]:3001`,
/// `*:3001`, or `127.0.0.1:3001`.
fn listeners_are_ipv6_only(lsof_stdout: &str) -> bool {
    let binds: Vec<&str> = lsof_stdout
        .lines()
        .skip(1) // header
        .filter_map(|line| {
            let mut columns = line.split_whitespace().rev();
            columns.next()?; // (LISTEN)
            columns.next()
        })
        .filter(|bind| bind.contains(':'))
        .collect();
    !binds.is_empty() && binds.iter().all(|bind| bind.starts_with("[::1]"))
}

/// Every linked host across every project, read from the Traefik
/// file-provider dir — the only global record of what's linked.
pub fn linked_hosts() -> Result<Vec<LinkedHost>> {
    let dyn_dir = paths::dynamic_projects_dir()?;
    if !dyn_dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in
        std::fs::read_dir(&dyn_dir).with_context(|| format!("reading {}", dyn_dir.display()))?
    {
        let path = entry?.path();
        if path.extension().and_then(|s| s.to_str()) != Some("yml") {
            continue;
        }
        let Some(slug) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        // `_henk.yml` is the dashboard / TLS config — not a linked project.
        if slug.starts_with('_') {
            continue;
        }
        let body = std::fs::read_to_string(&path).unwrap_or_default();
        out.extend(parse_linked_hosts(slug, &body));
    }
    out.sort_by(|a, b| (&a.slug, &a.host).cmp(&(&b.slug, &b.host)));
    Ok(out)
}

/// Pull `(host, backend url)` pairs out of one project's file-provider YAML,
/// in document order. Routers point at a service, which is either a `failover`
/// (current rendering — follow it to the primary) or a plain `loadBalancer`
/// (files written before the failover work landed).
pub fn parse_linked_hosts(slug: &str, body: &str) -> Vec<LinkedHost> {
    let Ok(doc) = serde_yaml_ng::from_str::<Value>(body) else {
        return Vec::new();
    };
    let http = &doc["http"];
    let Some(routers) = http["routers"].as_mapping() else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for (_, router) in routers {
        // Each host also has an http router that only redirects to https. It
        // names the same host and the same service, so counting it would list
        // and probe every linked host twice.
        if is_https_redirect(router) {
            continue;
        }
        let Some(host) = router["rule"].as_str().and_then(host_from_rule) else {
            continue;
        };
        let Some(service) = router["service"].as_str() else {
            continue;
        };
        if let Some(url) = resolve_backend_url(&http["services"], service) {
            out.push(LinkedHost {
                slug: slug.to_string(),
                host,
                url,
            });
        }
    }
    out
}

fn is_https_redirect(router: &Value) -> bool {
    router["middlewares"]
        .as_sequence()
        .is_some_and(|middlewares| {
            middlewares
                .iter()
                .any(|middleware| middleware.as_str() == Some("henk-https-redirect"))
        })
}

fn host_from_rule(rule: &str) -> Option<String> {
    let (_, rest) = rule.split_once("Host(`")?;
    let (host, _) = rest.split_once('`')?;
    Some(host.to_string())
}

fn resolve_backend_url(services: &Value, name: &str) -> Option<String> {
    let service = &services[name];
    if let Some(primary) = service["failover"]["service"].as_str() {
        return resolve_backend_url(services, primary);
    }
    service["loadBalancer"]["servers"][0]["url"]
        .as_str()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(plain: PortResponse, with_host: Option<u16>, ipv6_only_listener: bool) -> Observation {
        Observation {
            plain,
            with_host,
            ipv6_only_listener,
        }
    }

    #[test]
    fn ipv6_only_bind_is_a_blocker_with_the_fix_in_it() {
        let (status, detail) = classify(3001, "app.test", obs(PortResponse::Refused, None, true));
        assert_eq!(status, Status::Block);
        assert!(detail.contains("IPv6 loopback"));
        assert!(detail.contains("0.0.0.0"));
    }

    #[test]
    fn nothing_listening_is_a_warning_not_a_blocker() {
        let (status, detail) = classify(3001, "app.test", obs(PortResponse::Refused, None, false));
        assert_eq!(status, Status::Warn);
        assert!(detail.contains("nothing listening"));
    }

    #[test]
    fn ws_only_port_explains_the_426() {
        // The bug that prompted all this: the HMR socket holds the IPv4
        // wildcard while the app sits on [::1], so Traefik gets a 426.
        let (status, detail) =
            classify(3001, "app.test", obs(PortResponse::Code(426), None, false));
        assert_eq!(status, Status::Block);
        assert!(detail.contains("WebSocket"));
        assert!(detail.contains("0.0.0.0"));
    }

    #[test]
    fn rejected_hostname_points_at_allowed_hosts() {
        let (status, detail) = classify(
            3001,
            "calculation-tool.test",
            obs(PortResponse::Code(302), Some(403), false),
        );
        assert_eq!(status, Status::Block);
        assert!(detail.contains("calculation-tool.test"));
        assert!(detail.contains("allowedHosts"));
    }

    #[test]
    fn redirect_on_root_is_healthy() {
        let (status, _) = classify(
            3001,
            "app.test",
            obs(PortResponse::Code(302), Some(302), false),
        );
        assert_eq!(status, Status::Ok);
    }

    #[test]
    fn non_alive_status_suggests_health_path() {
        let (status, detail) =
            classify(8000, "app.test", obs(PortResponse::Code(401), None, false));
        assert_eq!(status, Status::Warn);
        assert!(detail.contains("health_path"));
    }

    #[test]
    fn lsof_all_ipv6_loopback_is_ipv6_only() {
        let out = "COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME\n\
                   node 85096 me 14u IPv6 0xe544 0t0 TCP [::1]:3001 (LISTEN)\n";
        assert!(listeners_are_ipv6_only(out));
    }

    #[test]
    fn lsof_with_a_wildcard_listener_is_not_ipv6_only() {
        let out = "COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME\n\
                   node 85096 me 14u IPv6 0xe544 0t0 TCP [::1]:3001 (LISTEN)\n\
                   node 85096 me 134u IPv6 0x6fed 0t0 TCP *:3001 (LISTEN)\n";
        assert!(!listeners_are_ipv6_only(out));
    }

    #[test]
    fn lsof_empty_is_not_ipv6_only() {
        assert!(!listeners_are_ipv6_only(""));
    }

    #[test]
    fn parses_hosts_and_follows_the_failover_to_the_primary() {
        let body = r#"
http:
  routers:
    calculation-tool:
      rule: "Host(`calculation-tool.test`)"
      service: calculation-tool
  services:
    calculation-tool:
      failover:
        service: calculation-tool-main
        fallback: henk-error-pages
    calculation-tool-main:
      loadBalancer:
        servers:
          - url: "http://host.docker.internal:3001"
"#;
        let hosts = parse_linked_hosts("calculation-tool", body);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].host, "calculation-tool.test");
        assert_eq!(hosts[0].url, "http://host.docker.internal:3001");
        assert_eq!(hosts[0].host_mode_port(), Some(3001));
    }

    #[test]
    fn the_https_redirect_router_is_not_a_second_linked_host() {
        // It names the same host and service as the real router, so counting it
        // would list and probe every linked host twice.
        let body = r#"
http:
  routers:
    calculation-tool:
      rule: "Host(`calculation-tool.test`)"
      service: calculation-tool
    calculation-tool-http:
      rule: "Host(`calculation-tool.test`)"
      entryPoints:
        - web
      middlewares:
        - henk-https-redirect
      service: calculation-tool
  services:
    calculation-tool:
      failover:
        service: calculation-tool-main
        fallback: henk-error-pages
    calculation-tool-main:
      loadBalancer:
        servers:
          - url: "http://host.docker.internal:3001"
"#;
        let hosts = parse_linked_hosts("calculation-tool", body);
        assert_eq!(hosts.len(), 1, "one linked host, not two");
    }

    #[test]
    fn parses_files_written_before_the_failover_rendering() {
        let body = r#"
http:
  routers:
    sparkle:
      rule: "Host(`sparkle.test`)"
      service: sparkle
  services:
    sparkle:
      loadBalancer:
        servers:
          - url: "http://host.docker.internal:3000"
"#;
        let hosts = parse_linked_hosts("sparkle", body);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].host_mode_port(), Some(3000));
    }

    #[test]
    fn docker_mode_backends_are_not_probed_from_the_host() {
        let body = r#"
http:
  routers:
    spatiebalk:
      rule: "Host(`spatiebalk.test`)"
      service: spatiebalk
  services:
    spatiebalk:
      loadBalancer:
        servers:
          - url: "http://laravel.test:80"
"#;
        let hosts = parse_linked_hosts("spatiebalk", body);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].host_mode_port(), None);
    }
}
