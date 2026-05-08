//! Top-level CLI definition and dispatch.

use anyhow::Result;
use clap::{Parser, Subcommand};

mod dashboard;
mod default;
mod doctor;
mod down;
mod init;
mod link;
mod status;
mod uninstall;
mod unlink;
mod up;
mod update;

/// `henk` — local-dev URL routing for Docker on macOS.
#[derive(Debug, Parser)]
#[command(
    name = "henk",
    version,
    about = "Local-dev URL routing for Docker on macOS",
    long_about = "henk turns any Docker container (or local dev server) into \
                  https://<name>.test with a trusted certificate, no \
                  /etc/hosts edits, and no nginx config."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// First-run setup: detect prerequisites, install missing pieces, write
    /// config, bring up the global Traefik+dnsmasq stack.
    Init {
        /// Run all detection steps but don't make any system changes.
        #[arg(long)]
        dry_run: bool,

        /// Override the auto-picked TLD (default: `.test`, or `.henk` if
        /// Valet/Herd is detected).
        #[arg(long)]
        tld: Option<String>,

        /// Skip prompts; assume Yes for every consent step. Requires sudo to
        /// be primed (e.g. via SUDO_ASKPASS) for non-interactive runs.
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Register the project in the current directory.
    Link {
        /// Add another hostname to an already-linked project rather than
        /// re-linking from scratch.
        #[arg(long)]
        add: bool,

        /// Override the auto-detected hostname.
        #[arg(long)]
        host: Option<String>,

        /// Override the auto-detected service (Docker mode). Useful for
        /// multi-service projects where `--add` should route a sub-host
        /// to a different container — e.g. `--service mailhog --port 8025`.
        #[arg(long)]
        service: Option<String>,

        /// Override the auto-detected container port. Pairs with
        /// `--service` for unambiguous overrides.
        #[arg(long)]
        port: Option<u16>,
    },

    /// Remove a project (or one of its hosts) from routing.
    Unlink {
        /// Specific host to remove. If omitted, removes the entire project.
        host: Option<String>,
    },

    /// Show stack health, linked projects, certs.
    Status,

    /// Start the global Traefik + dnsmasq stack.
    Up,

    /// Stop the global Traefik + dnsmasq stack (and keep it stopped).
    Down,

    /// Run all detection + health checks. `--repair` re-runs failed init
    /// steps surgically.
    Doctor {
        #[arg(long)]
        repair: bool,
    },

    /// Self-update the henk binary from GitHub Releases.
    Update {
        /// Print whether a newer version is available without installing.
        #[arg(long)]
        check: bool,
    },

    /// Reverse what henk has done. Default removes only henk's own files;
    /// `--deep` also removes Homebrew packages henk installed.
    Uninstall {
        #[arg(long)]
        deep: bool,
        /// Stop the stack but keep `~/.config/henk/` for a future re-init.
        #[arg(long)]
        keep_config: bool,
    },

    /// Live TUI: stack health, linked projects, certificate state.
    Dashboard,
}

/// Entry point called by `main`. Parses already-done; just dispatches.
pub async fn dispatch(cli: Cli) -> Result<()> {
    match cli.command {
        None => default::run().await,
        Some(Command::Init { dry_run, tld, yes }) => init::run(dry_run, tld, yes).await,
        Some(Command::Link { add, host, service, port }) => {
            link::run(add, host, service, port).await
        }
        Some(Command::Unlink { host }) => unlink::run(host).await,
        Some(Command::Status) => status::run().await,
        Some(Command::Up) => up::run().await,
        Some(Command::Down) => down::run().await,
        Some(Command::Doctor { repair }) => doctor::run(repair).await,
        Some(Command::Update { check }) => update::run(check).await,
        Some(Command::Uninstall { deep, keep_config }) => {
            uninstall::run(deep, keep_config).await
        }
        Some(Command::Dashboard) => dashboard::run().await,
    }
}
