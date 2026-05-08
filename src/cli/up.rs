use anyhow::{Result, bail};

use crate::config::Config;
use crate::runner::SystemRunner;
use crate::stack::lifecycle;

pub async fn run() -> Result<()> {
    let runner = SystemRunner::new();
    let cfg = match Config::load()? {
        Some(c) => c,
        None => bail!("henk has not been initialised yet. Run `henk init` first."),
    };
    lifecycle::up(&runner, &cfg).await
}
