//! `henk init` — detect prerequisites, render the table, and (post-M1) wire up
//! the system. M1 only implements `--dry-run`: full detection + table render,
//! no system writes.

use anyhow::{Result, bail};

use crate::detect::{self, DetectionReport};
use crate::runner::SystemRunner;

pub async fn run(dry_run: bool, tld: Option<String>, _yes: bool) -> Result<()> {
    if !dry_run {
        bail!(
            "henk init (full mode) is not yet implemented (M5).\n\
             Run `henk init --dry-run` to see the detection report."
        );
    }

    let runner = SystemRunner::new();
    let report: DetectionReport = detect::run_all(&runner, tld.as_deref()).await?;
    report.print();

    Ok(())
}
