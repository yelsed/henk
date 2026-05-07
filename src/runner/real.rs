//! Real implementation of command execution.

use anyhow::{Context, Result};
use std::ffi::OsStr;
use tokio::process::Command;

#[derive(Debug, Clone, Default)]
pub struct SystemRunner;

impl SystemRunner {
    pub fn new() -> Self {
        Self
    }

    /// Run a program with the given arguments and capture stdout/stderr.
    /// Does not propagate non-zero exit as an error — callers inspect
    /// `CommandOutput::ok()` themselves.
    pub async fn run<I, S>(&self, program: &str, args: I) -> Result<CommandOutput>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = Command::new(program)
            .args(args)
            .output()
            .await
            .with_context(|| format!("failed to execute `{program}`"))?;

        Ok(CommandOutput {
            status: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    /// Convenience: returns true if `which <program>` succeeds.
    pub async fn which(&self, program: &str) -> bool {
        match Command::new("which").arg(program).output().await {
            Ok(o) => o.status.success(),
            Err(_) => false,
        }
    }

    /// Convenience: returns true if running `program args...` exits 0.
    pub async fn ok<I, S>(&self, program: &str, args: I) -> bool
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        match self.run(program, args).await {
            Ok(o) => o.ok(),
            Err(_) => false,
        }
    }

    /// Run a program with stdin/stdout/stderr inherited from the current
    /// process. Used for interactive prompts (sudo password, mkcert keychain
    /// confirmation, etc.). Returns the exit status; output is not captured.
    pub async fn run_inherit<I, S>(&self, program: &str, args: I) -> anyhow::Result<i32>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        use std::process::Stdio;
        let status = Command::new(program)
            .args(args)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .await
            .with_context(|| format!("failed to execute `{program}`"))?;
        Ok(status.code().unwrap_or(-1))
    }
}

#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    pub fn ok(&self) -> bool {
        self.status == 0
    }

    /// Returns the first non-empty line of stdout, trimmed.
    pub fn first_line(&self) -> Option<&str> {
        self.stdout.lines().map(str::trim).find(|l| !l.is_empty())
    }
}
