//! Detection probes. Reads the host system and the current project to
//! produce a `DetectionReport` that drives the wizard and the
//! `henk init --dry-run` table.
//!
//! All probes are read-only.

use anyhow::Result;

use crate::runner::SystemRunner;

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
            let icon = match item.status {
                Status::Ok => "✓".green().to_string(),
                Status::Warn => "!".yellow().to_string(),
                Status::Block => "✗".red().to_string(),
                Status::Info => "i".bright_black().to_string(),
            };
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
            println!(
                "{}",
                "No blockers. `henk init` would proceed (full mode lands in M5)."
                    .green()
            );
        }
        println!();
    }
}

/// Run the full detection suite. `tld_override` is `Some` when the user
/// passed `--tld <foo>`, in which case we still record findings but don't
/// derive the TLD ourselves.
pub async fn run_all(
    runner: &SystemRunner,
    tld_override: Option<&str>,
) -> Result<DetectionReport> {
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
    items.push(resolver::probe(&tld.value()));

    // Ports we plan to bind.
    items.push(ports::probe_port(runner, "host TCP :80", 80, "http").await);
    items.push(ports::probe_port(runner, "host TCP :443", 443, "https").await);
    // Port :53 lives outside the henk stack — it's bound by Homebrew dnsmasq
    // under launchd. The dnsmasq install path handles the start/restart
    // dance (sharing the slot with Valet/DDEV via the dnsmasq.d/ drop-in
    // pattern is fine), so we don't probe it here.

    // Existing henk-proxy network or foreign Traefik containers.
    items.push(docker::probe_proxy_network(runner).await);
    items.push(docker::probe_foreign_traefik(runner).await);

    Ok(DetectionReport { items, tld })
}
