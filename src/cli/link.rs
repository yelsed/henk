use anyhow::{Context, Result};

use crate::project;
use crate::runner::SystemRunner;

pub async fn run(
    add: bool,
    host: Option<String>,
    service: Option<String>,
    port: Option<u16>,
    health_path: Option<String>,
) -> Result<()> {
    let runner = SystemRunner::new();
    let cwd = std::env::current_dir().context("could not resolve current working directory")?;
    project::link::run(&runner, &cwd, add, host, service, port, health_path).await
}
