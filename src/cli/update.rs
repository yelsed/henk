//! `henk update` — self-update from GitHub Releases via axoupdater.
//!
//! `henk update`         — check for a newer version and install it if found.
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

use anyhow::Result;
use axoupdater::{AxoUpdater, UpdateRequest};
use owo_colors::OwoColorize;

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
        }
        Ok(None) => {
            println!("  {}  Already on the latest version.", "✓".bright_green());
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
