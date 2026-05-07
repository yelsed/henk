//! Port-binding probes via `lsof`. Detect what (if anything) is listening
//! on the host ports henk wants to bind.

use crate::detect::{DetectionItem, Status};
use crate::runner::SystemRunner;

/// Probe a single TCP port. `name` is the human-readable label (e.g.
/// "host TCP :80"); `purpose` is what henk wants to use it for (e.g. "http").
pub async fn probe_port(
    runner: &SystemRunner,
    name: &'static str,
    port: u16,
    purpose: &str,
) -> DetectionItem {
    let arg = format!("-iTCP:{port}");
    let out = runner
        .run("lsof", ["-nP", "-sTCP:LISTEN", "-F", "pcn", &arg])
        .await;
    match out {
        Ok(o) if o.ok() && !o.stdout.trim().is_empty() => {
            // lsof -F output: lines like `pPID`, `cCMD`, `nADDR`. Pick the first
            // PID/cmd we see.
            let mut pid = None::<&str>;
            let mut cmd = None::<&str>;
            for line in o.stdout.lines() {
                if let Some(rest) = line.strip_prefix('p') {
                    pid = Some(rest);
                } else if let Some(rest) = line.strip_prefix('c') {
                    cmd = Some(rest);
                    break;
                }
            }
            let cmd = cmd.unwrap_or("unknown");
            let pid = pid.unwrap_or("?");
            DetectionItem {
                name,
                status: Status::Block,
                detail: format!(
                    "in use by `{cmd}` (pid {pid}) — needed for {purpose}; stop it or pick another port",
                ),
            }
        }
        Ok(_) => DetectionItem {
            name,
            status: Status::Ok,
            detail: format!("free (will be used for {purpose})"),
        },
        Err(_) => DetectionItem {
            name,
            status: Status::Warn,
            detail: "could not invoke `lsof` — is it installed?".into(),
        },
    }
}
