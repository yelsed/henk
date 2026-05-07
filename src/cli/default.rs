//! `henk` (no args) — smart context-aware status.
//!
//! Outside a linked project: stack + linked-project summary.
//! Inside one: project-specific routing + health.
//!
//! M1 stub: prints a minimal version + nudge to `henk init`.

use anyhow::Result;

pub async fn run() -> Result<()> {
    let version = env!("CARGO_PKG_VERSION");
    println!("henk {version} — local-dev URL routing for Docker");
    println!();
    println!("Not yet initialised. Run `henk init` to set up the global stack.");
    println!("`henk --help` to list commands.");
    Ok(())
}
