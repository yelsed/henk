//! Port-binding probes. Detect what (if anything) is listening on the host
//! ports henk wants to bind: `lsof` on macOS, `ss` on Linux — where `lsof`
//! can't see sockets owned by other users (docker-proxy, root daemons).

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
    if cfg!(target_os = "linux") {
        return probe_port_ss(runner, name, port, purpose).await;
    }
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

async fn probe_port_ss(
    runner: &SystemRunner,
    name: &'static str,
    port: u16,
    purpose: &str,
) -> DetectionItem {
    let filter = format!(":{port}");
    match runner.run("ss", ["-Hltnp", "sport", "=", &filter]).await {
        Ok(out) if out.ok() => match loopback_conflict(&out.stdout) {
            Some(holder) => DetectionItem {
                name,
                status: Status::Block,
                detail: format!(
                    "in use by {holder} — needed for {purpose}; stop it or pick another port"
                ),
            },
            None => DetectionItem {
                name,
                status: Status::Ok,
                detail: format!("free on 127.0.0.1 (will be used for {purpose})"),
            },
        },
        _ => DetectionItem {
            name,
            status: Status::Warn,
            detail: "could not invoke `ss` — is iproute2 installed?".into(),
        },
    }
}

/// On Linux henk publishes on 127.0.0.1 only, so a listener conflicts only
/// when it holds loopback or a wildcard. One bound to a single other address
/// (`tailscale serve` on the tailnet IP) leaves 127.0.0.1 free. Returns who
/// holds it; `ss` names the process only when it's ours.
fn loopback_conflict(ss_stdout: &str) -> Option<String> {
    ss_stdout.lines().find_map(|line| {
        let columns: Vec<&str> = line.split_whitespace().collect();
        let local = columns.get(3)?;
        let (address, _port) = local.rsplit_once(':')?;
        let address = address.split('%').next().unwrap_or(address);
        if !matches!(
            address,
            "0.0.0.0" | "*" | "[::]" | "127.0.0.1" | "[::ffff:127.0.0.1]"
        ) {
            return None;
        }
        let process = columns
            .get(5)
            .and_then(|users| users.split('"').nth(1))
            .map(|command| format!("`{command}` on {local}"));
        Some(process.unwrap_or_else(|| format!("another user's process on {local}")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listener_on_one_other_address_leaves_loopback_free() {
        let out = "LISTEN 0 4096 100.121.60.36:443 0.0.0.0:*\n\
                   LISTEN 0 4096 [fd7a:115c:a1e0::9035:3c24]:443 [::]:*\n";
        assert_eq!(loopback_conflict(out), None);
    }

    #[test]
    fn wildcard_and_loopback_listeners_conflict() {
        for local in ["0.0.0.0:80", "*:80", "[::]:80", "127.0.0.1:80"] {
            let out = format!("LISTEN 0 4096 {local} 0.0.0.0:*\n");
            assert!(loopback_conflict(&out).is_some(), "{local}");
        }
    }

    #[test]
    fn names_the_process_when_ss_can_see_it() {
        let out = "LISTEN 0 511 0.0.0.0:80 0.0.0.0:* users:((\"nginx\",pid=42,fd=6))\n";
        assert_eq!(
            loopback_conflict(out).as_deref(),
            Some("`nginx` on 0.0.0.0:80")
        );
    }

    #[test]
    fn nothing_listening_is_free() {
        assert_eq!(loopback_conflict(""), None);
    }
}
