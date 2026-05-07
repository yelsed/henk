use anyhow::{Context, Result};

use crate::project;
use crate::runner::SystemRunner;

pub async fn run(add: bool, host: Option<String>) -> Result<()> {
    let runner = SystemRunner::new();
    let cwd = std::env::current_dir().context("could not resolve current working directory")?;
    project::link::run(&runner, &cwd, add, host).await
}
