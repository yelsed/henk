//! `henk dashboard` — opens the live TUI in `tui::dashboard`.
//!
//! The TUI loop is synchronous (crossterm + ratatui) so we hop off the
//! tokio runtime via `spawn_blocking`. That keeps async runtime
//! responsive (it's idle while the dashboard owns the terminal anyway)
//! and stops crossterm's blocking polls from starving other tasks.

use anyhow::{Context, Result};

pub async fn run() -> Result<()> {
    let join = tokio::task::spawn_blocking(crate::tui::dashboard::run);
    join.await.context("dashboard task panicked")?
}
