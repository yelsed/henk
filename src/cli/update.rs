//! `henk update` — self-update from GitHub Releases via axoupdater, then bring
//! the stack up to whatever the new binary ships.
//!
//! `henk update`         — check for a newer version, install it if found, and
//!                         re-render + restart the stack so the running proxy
//!                         matches the new binary.
//! `henk update --check` — print whether a newer version is available without
//!                         installing anything.
//!
//! Both paths write an audit entry to state.json so the ops log records that
//! the user explicitly asked. That's useful when diagnosing reports of
//! "I ran `henk update`, then …".
//!
//! axoupdater reads a cargo-dist install receipt
//! (`~/.config/henk/install_receipt.json`) to know the current version and
//! release source. When the receipt is absent (e.g. the binary was built from
//! source), we fall back gracefully rather than panicking.

use anyhow::{Context, Result};
use axoupdater::{AxoUpdater, UpdateRequest};
use owo_colors::OwoColorize;
use std::process::Command;

use crate::manifest::StateManifest;

pub async fn run(check: bool) -> Result<()> {
    // Always audit so the ops log captures the intent, regardless of outcome.
    if let Some(mut state) = StateManifest::load()? {
        state.audit(
            if check {
                "henk update --check"
            } else {
                "henk update"
            },
            0,
        );
        state.save()?;
    }

    println!();
    println!("{}", "henk — update".bold());
    println!();

    // Build the updater. load_receipt() reads the cargo-dist install receipt
    // written by the installer script at ~/.config/henk/install_receipt.json.
    // If the receipt is missing the binary was built from source; we surface
    // a clear message instead of an opaque error.
    let mut updater = AxoUpdater::new_for("henk");
    if let Err(e) = updater.load_receipt() {
        println!(
            "  {}  Could not load install receipt: {}",
            "·".bright_black(),
            e
        );
        println!();
        println!("  This usually means henk was built from source rather than");
        println!("  installed via the release installer. To update, pull the");
        println!("  latest source and rebuild:");
        println!();
        println!("    git pull && cargo build --release");
        println!();
        return Ok(());
    }

    if check {
        // --check: query the latest release without installing.
        match updater.is_update_needed().await {
            Ok(true) => {
                println!(
                    "  {}  A newer version of henk is available.",
                    "↑".bright_green()
                );
                println!("  Run {} to install it.", "henk update".bold());
            }
            Ok(false) => {
                println!("  {}  henk is up to date.", "✓".bright_green());
            }
            Err(e) => {
                println!(
                    "  {}  Could not check for updates: {}",
                    "·".bright_black(),
                    e
                );
                println!();
                println!("  Check https://github.com/fivespark/henk/releases manually.");
            }
        }
        println!();
        return Ok(());
    }

    // Bare `henk update`: fetch and install the latest release if newer.
    updater.configure_version_specifier(UpdateRequest::Latest);

    match updater.run().await {
        Ok(Some(result)) => {
            println!(
                "  {}  Updated to {}",
                "✓".bright_green(),
                result.new_version.to_string().bold()
            );
            upgrade_stack()?;
        }
        Ok(None) => {
            println!("  {}  Already on the latest version.", "✓".bright_green());
            // The binary can reach "latest" without henk having installed it —
            // Homebrew, the installer script, `cargo install`. The stack is then
            // still running whatever the *previous* binary rendered, and `up` is
            // a no-op when it isn't.
            upgrade_stack()?;
        }
        Err(e) => {
            // Surface the error but don't hard-fail — the user can still
            // download manually. Record the failure in the audit log.
            if let Some(mut state) = StateManifest::load()? {
                state.audit("henk update (failed)", 1);
                state.save()?;
            }
            println!("  {}  Update failed: {}", "✗".bright_red(), e);
            println!();
            println!("  Download manually from https://github.com/fivespark/henk/releases");
        }
    }

    println!();
    Ok(())
}

/// A new binary ships new stack templates, but the containers keep running the
/// ones they booted with — an updated henk with a v-old proxy routes by the old
/// rules until someone re-renders. So bring the stack up as part of updating.
///
/// This has to be done by the *new* binary: the process running right now is the
/// old one, and it can only ever render the templates compiled into it. The
/// installer replaced the file at our own path, so re-invoking it runs the new
/// build.
fn upgrade_stack() -> Result<()> {
    let exe = std::env::current_exe().context("locating the henk binary we were run from")?;

    println!();
    println!(
        "  {}  Bringing the stack up to date ...",
        "⤷".bright_black()
    );
    println!();

    let status = Command::new(&exe).arg("up").status();

    match status {
        Ok(status) if status.success() => Ok(()),
        _ => {
            println!(
                "  {}  The new henk is installed, but the stack wasn't upgraded.",
                "!".yellow()
            );
            println!("  Run {} to finish.", "henk up".bold());
            println!();
            Ok(())
        }
    }
}
