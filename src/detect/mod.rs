//! Detection probes. Reads the host system and the current project to
//! produce a `DetectionReport` that drives the wizard and the
//! `henk init --dry-run` table.
//!
//! All probes are read-only.

use anyhow::Result;

use crate::runner::SystemRunner;

pub mod backend;
mod brew;
mod coexistence;
mod docker;
pub mod ports;
mod resolver;
mod tld;

pub use tld::{TldChoice, TldReason};

/// One row in the detection table.
#[derive(Debug, Clone)]
pub struct DetectionItem {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Healthy / present. Green ✓.
    Ok,
    /// Worth flagging but doesn't block init. Yellow !.
    Warn,
    /// Hard collision — init must abort. Red ✗.
    Block,
    /// Pure informational note. Gray i.
    Info,
}

impl Status {
    /// The status glyph, coloured. Shared so every surface (detection table,
    /// doctor, status) speaks the same visual language.
    pub fn glyph(self) -> String {
        use owo_colors::OwoColorize;
        match self {
            Status::Ok => "✓".green().to_string(),
            Status::Warn => "!".yellow().to_string(),
            Status::Block => "✗".red().to_string(),
            Status::Info => "i".bright_black().to_string(),
        }
    }
}

/// The whole detection report.
#[derive(Debug, Clone)]
pub struct DetectionReport {
    pub items: Vec<DetectionItem>,
    pub tld: TldChoice,
}

impl DetectionReport {
    /// Did any probe report a `Block` status?
    pub fn has_blockers(&self) -> bool {
        self.items.iter().any(|i| i.status == Status::Block)
    }

    /// Pretty-print to stdout. Used by `henk init --dry-run` and by the
    /// wizard's first screen.
    pub fn print(&self) {
        use owo_colors::OwoColorize;

        println!();
        println!("{}", "henk — environment detection".bold());
        println!();

        // Compute padding so detail columns line up.
        let max_name = self.items.iter().map(|i| i.name.len()).max().unwrap_or(0);

        for item in &self.items {
            let icon = item.status.glyph();
            let name = format!("{:width$}", item.name, width = max_name);
            println!("  {icon}  {name}   {}", item.detail);
        }

        println!();
        println!("  {}", self.tld.summary());
        println!();

        if self.has_blockers() {
            println!(
                "{}",
                "Blockers above must be resolved before `henk init` can proceed."
                    .red()
                    .bold()
            );
        } else {
            println!("{}", "No blockers — `henk init` is ready to run.".green());
        }
        println!();
    }
}

/// Run the full detection suite. `tld_override` is `Some` when the user
/// passed `--tld <foo>`, in which case we still record findings but don't
/// derive the TLD ourselves.
pub async fn run_all(runner: &SystemRunner, tld_override: Option<&str>) -> Result<DetectionReport> {
    let mut items = Vec::new();

    // Prerequisites.
    items.push(docker::probe(runner).await);
    items.push(brew::probe_homebrew(runner).await);
    items.push(brew::probe_mkcert(runner).await);
    items.push(brew::probe_nss(runner).await);
    items.push(brew::probe_dnsmasq(runner).await);

    // Coexistence with other dev tools.
    let valet_present = coexistence::valet_detected(runner).await;
    let herd_present = coexistence::herd_detected(runner).await;
    items.push(coexistence::valet_item(valet_present));
    items.push(coexistence::herd_item(herd_present));
    items.push(coexistence::ddev_item(runner).await);
    items.push(coexistence::lando_item(runner).await);

    // Decide the TLD up front so port + resolver checks know what to look for.
    let tld = tld::decide(tld_override, valet_present, herd_present);

    // Existing resolver file for our chosen TLD.
    items.push(resolver::probe(tld.value()));

    // Ports we plan to bind. If our own Traefik is already bound to
    // these, downgrade the Block to an Info — re-running `henk init` on
    // a host where the stack is already up shouldn't be a hard fail.
    let henk_traefik_ports = henk_traefik_published_ports(runner).await;
    let mut p80 = ports::probe_port(runner, "host TCP :80", 80, "http").await;
    let mut p443 = ports::probe_port(runner, "host TCP :443", 443, "https").await;
    for (item, port) in [(&mut p80, 80u16), (&mut p443, 443u16)] {
        if item.status == Status::Block && henk_traefik_ports.contains(&port) {
            item.status = Status::Info;
            item.detail = "bound by henk-traefik (will be reused)".to_string();
        }
    }
    items.push(p80);
    items.push(p443);
    // Port :53 lives outside the henk stack — it's bound by Homebrew dnsmasq
    // under launchd. The dnsmasq install path handles the start/restart
    // dance (sharing the slot with Valet/DDEV via the dnsmasq.d/ drop-in
    // pattern is fine), so we don't probe it here.

    // Existing henk-proxy network or foreign Traefik containers.
    items.push(docker::probe_proxy_network(runner).await);
    items.push(docker::probe_foreign_traefik(runner).await);

    Ok(DetectionReport { items, tld })
}

/// Host ports the running `henk-traefik` container publishes. Empty when
/// the container isn't running. Used to recognise our own port bindings
/// during re-init so they don't show up as collisions.
async fn henk_traefik_published_ports(runner: &SystemRunner) -> Vec<u16> {
    let out = runner
        .run(
            "docker",
            [
                "ps",
                "--filter",
                "name=henk-traefik",
                "--format",
                "{{.Ports}}",
            ],
        )
        .await;
    let Ok(o) = out else { return Vec::new() };
    if !o.ok() {
        return Vec::new();
    }
    // `docker ps --format {{.Ports}}` returns lines like
    // `0.0.0.0:80->80/tcp, 127.0.0.1:19080->8080/tcp`. We just want the
    // host-side port numbers (the segment immediately before `->`).
    let mut ports = Vec::new();
    for chunk in o.stdout.split([',', ' ', '\n']) {
        let chunk = chunk.trim();
        let Some((host_part, _container_part)) = chunk.split_once("->") else {
            continue;
        };
        let port_str = host_part.rsplit(':').next().unwrap_or(host_part);
        if let Ok(p) = port_str.parse::<u16>() {
            ports.push(p);
        }
    }
    ports
}
