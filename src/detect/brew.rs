//! Homebrew-package probes.

use crate::detect::{DetectionItem, Status};
use crate::runner::SystemRunner;

pub async fn probe_homebrew(runner: &SystemRunner) -> DetectionItem {
    if !runner.which("brew").await {
        return DetectionItem {
            name: "Homebrew",
            status: Status::Block,
            detail: "not installed (https://brew.sh)".into(),
        };
    }
    match runner.run("brew", ["--version"]).await {
        Ok(out) if out.ok() => DetectionItem {
            name: "Homebrew",
            status: Status::Ok,
            detail: out.first_line().unwrap_or("installed").to_string(),
        },
        _ => DetectionItem {
            name: "Homebrew",
            status: Status::Warn,
            detail: "found in $PATH but `brew --version` failed".into(),
        },
    }
}

pub async fn probe_mkcert(runner: &SystemRunner) -> DetectionItem {
    probe_pkg(runner, "mkcert").await
}

pub async fn probe_nss(runner: &SystemRunner) -> DetectionItem {
    probe_pkg(runner, "nss").await
}

pub async fn probe_dnsmasq(runner: &SystemRunner) -> DetectionItem {
    probe_pkg(runner, "dnsmasq").await
}

async fn probe_pkg(runner: &SystemRunner, pkg: &'static str) -> DetectionItem {
    // Cheap path: is it in $PATH?
    if runner.which(pkg).await {
        return DetectionItem {
            name: pkg,
            status: Status::Ok,
            detail: "installed".into(),
        };
    }
    // Fall back to `brew list <pkg>` for libraries (nss isn't on PATH).
    if runner.ok("brew", ["list", "--versions", pkg]).await {
        return DetectionItem {
            name: pkg,
            status: Status::Ok,
            detail: "installed (via brew)".into(),
        };
    }
    DetectionItem {
        name: pkg,
        status: Status::Warn,
        detail: format!("missing — `henk init` will offer `brew install {pkg}`"),
    }
}
