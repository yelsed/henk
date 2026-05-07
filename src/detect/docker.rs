//! Docker-related probes.

use crate::consts::PROXY_NETWORK;
use crate::detect::{DetectionItem, Status};
use crate::runner::SystemRunner;

/// Is Docker installed and the daemon reachable?
pub async fn probe(runner: &SystemRunner) -> DetectionItem {
    if !runner.which("docker").await {
        return DetectionItem {
            name: "Docker",
            status: Status::Block,
            detail: "not found in $PATH (install Docker Desktop)".into(),
        };
    }
    match runner.run("docker", ["info", "--format", "{{.ServerVersion}}"]).await {
        Ok(out) if out.ok() => {
            let version = out.first_line().unwrap_or("unknown").to_string();
            DetectionItem {
                name: "Docker",
                status: Status::Ok,
                detail: format!("running ({version})"),
            }
        }
        Ok(_) => DetectionItem {
            name: "Docker",
            status: Status::Block,
            detail: "installed but daemon not reachable (start Docker Desktop)".into(),
        },
        Err(_) => DetectionItem {
            name: "Docker",
            status: Status::Block,
            detail: "could not invoke `docker info`".into(),
        },
    }
}

/// Does the `henk-proxy` network already exist? If so, is it ours?
pub async fn probe_proxy_network(runner: &SystemRunner) -> DetectionItem {
    let out = runner
        .run(
            "docker",
            [
                "network",
                "ls",
                "--filter",
                &format!("name={PROXY_NETWORK}"),
                "--format",
                "{{.Name}}",
            ],
        )
        .await;
    match out {
        Ok(o) if o.ok() => {
            let exists = o
                .stdout
                .lines()
                .any(|l| l.trim() == PROXY_NETWORK);
            if exists {
                DetectionItem {
                    name: "henk-proxy network",
                    status: Status::Info,
                    detail: "exists (will be reused if it's ours, else aborts)".into(),
                }
            } else {
                DetectionItem {
                    name: "henk-proxy network",
                    status: Status::Ok,
                    detail: "absent (will be created)".into(),
                }
            }
        }
        _ => DetectionItem {
            name: "henk-proxy network",
            status: Status::Warn,
            detail: "could not query Docker networks".into(),
        },
    }
}

/// Are there any other Traefik containers running on the host?
pub async fn probe_foreign_traefik(runner: &SystemRunner) -> DetectionItem {
    let out = runner
        .run(
            "docker",
            [
                "ps",
                "--filter",
                "ancestor=traefik",
                "--format",
                "{{.Names}}",
            ],
        )
        .await;
    match out {
        Ok(o) if o.ok() => {
            let names: Vec<&str> = o.stdout.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
            if names.is_empty() {
                DetectionItem {
                    name: "foreign Traefik",
                    status: Status::Ok,
                    detail: "none running".into(),
                }
            } else {
                DetectionItem {
                    name: "foreign Traefik",
                    status: Status::Warn,
                    detail: format!("found: {} (may compete for ports)", names.join(", ")),
                }
            }
        }
        _ => DetectionItem {
            name: "foreign Traefik",
            status: Status::Info,
            detail: "could not enumerate (Docker probe failed)".into(),
        },
    }
}
