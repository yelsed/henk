use anyhow::Result;

use crate::runner::SystemRunner;
use crate::stack::lifecycle;

pub async fn run() -> Result<()> {
    let runner = SystemRunner::new();
    lifecycle::down(&runner).await
}
