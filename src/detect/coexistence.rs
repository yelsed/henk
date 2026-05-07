//! Detection of other local-dev tools (Valet, Herd, DDEV, Lando) so henk
//! coexists rather than fighting them for `.test` / port 80 / port 443.

use std::path::Path;

use crate::detect::{DetectionItem, Status};
use crate::runner::SystemRunner;

/// Returns true if Laravel Valet appears to be installed on this machine.
pub async fn valet_detected(runner: &SystemRunner) -> bool {
    valet_dir_exists() || runner.which("valet").await
}

/// Returns true if Laravel Herd appears to be installed.
pub async fn herd_detected(runner: &SystemRunner) -> bool {
    Path::new("/Applications/Herd.app").exists() || runner.which("herd").await
}

fn valet_dir_exists() -> bool {
    if let Some(home) = dirs::home_dir() {
        if home.join(".config/valet").exists() {
            return true;
        }
    }
    false
}

pub fn valet_item(detected: bool) -> DetectionItem {
    if detected {
        DetectionItem {
            name: "Laravel Valet",
            status: Status::Warn,
            detail: "detected — henk will avoid `.test` to coexist".into(),
        }
    } else {
        DetectionItem {
            name: "Laravel Valet",
            status: Status::Ok,
            detail: "not detected".into(),
        }
    }
}

pub fn herd_item(detected: bool) -> DetectionItem {
    if detected {
        DetectionItem {
            name: "Laravel Herd",
            status: Status::Warn,
            detail: "detected — henk will avoid `.test` to coexist".into(),
        }
    } else {
        DetectionItem {
            name: "Laravel Herd",
            status: Status::Ok,
            detail: "not detected".into(),
        }
    }
}

pub async fn ddev_item(runner: &SystemRunner) -> DetectionItem {
    let cli = runner.which("ddev").await;
    let dir = dirs::home_dir()
        .map(|h| h.join(".ddev").exists())
        .unwrap_or(false);
    match (cli, dir) {
        (true, _) | (_, true) => DetectionItem {
            name: "DDEV",
            status: Status::Info,
            detail: "detected — fine unless it's running on :80/:443".into(),
        },
        _ => DetectionItem {
            name: "DDEV",
            status: Status::Ok,
            detail: "not detected".into(),
        },
    }
}

pub async fn lando_item(runner: &SystemRunner) -> DetectionItem {
    let cli = runner.which("lando").await;
    let dir = dirs::home_dir()
        .map(|h| h.join(".lando").exists())
        .unwrap_or(false);
    match (cli, dir) {
        (true, _) | (_, true) => DetectionItem {
            name: "Lando",
            status: Status::Info,
            detail: "detected — fine unless it's running on :80/:443".into(),
        },
        _ => DetectionItem {
            name: "Lando",
            status: Status::Ok,
            detail: "not detected".into(),
        },
    }
}
