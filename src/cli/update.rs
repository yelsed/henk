//! `henk update` — self-update from GitHub Releases.
//!
//! The actual update path runs via cargo-dist's `axoupdater`, which
//! requires the project to be shipping signed binaries via GitHub
//! Releases. That pipeline is set up in M9; until then `update`
//! refuses to do anything destructive and `update --check` prints a
//! pointer to the release page (when one exists).
//!
//! State manifest still gets a touch so the audit log records that the
//! user explicitly asked, which is useful when diagnosing reports
//! about "I ran `henk update`, then …".

use anyhow::Result;

use crate::manifest::StateManifest;

pub async fn run(check: bool) -> Result<()> {
    use owo_colors::OwoColorize;

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

    if check {
        println!(
            "  Self-update wiring lands when henk ships signed binaries via"
        );
        println!("  GitHub Releases (cargo-dist + axoupdater integration in M9).");
        println!();
        println!(
            "  For now, build from source: `cargo install --path .` from the repo."
        );
        return Ok(());
    }

    println!(
        "  {}  Self-update isn't available yet — see `henk update --check` for context.",
        "·".bright_black()
    );
    println!();
    Ok(())
}
