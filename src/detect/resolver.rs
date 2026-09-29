//! Detect an existing resolver file for the TLD — `/etc/resolver/<tld>` on
//! macOS, the systemd-resolved drop-in on Linux — so henk never silently
//! overwrites a Valet/Herd/manual resolver.

use std::fs;

use crate::consts::HENK_FILE_HEADER;
use crate::detect::{DetectionItem, Status};
use crate::stack::resolver::resolver_path;

pub fn probe(tld: &str) -> DetectionItem {
    let name = if cfg!(target_os = "linux") {
        "resolved drop-in"
    } else {
        "/etc/resolver/<tld>"
    };
    let path = resolver_path(tld);
    let shown = path.display();
    if !path.exists() {
        return DetectionItem {
            name,
            status: Status::Ok,
            detail: format!("{shown} absent (will be created with sudo)"),
        };
    }
    match fs::read_to_string(&path) {
        Ok(contents) if contents.contains(HENK_FILE_HEADER) => DetectionItem {
            name,
            status: Status::Info,
            detail: format!("{shown} exists (managed by henk; reused)"),
        },
        Ok(_) => DetectionItem {
            name,
            status: Status::Block,
            detail: format!("{shown} exists but isn't ours — pick a different TLD with --tld"),
        },
        Err(_) => DetectionItem {
            name,
            status: Status::Warn,
            detail: format!("{shown} exists but is unreadable"),
        },
    }
}
