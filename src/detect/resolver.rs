//! Detect existing `/etc/resolver/<tld>` files so henk never silently
//! overwrites a Valet/Herd/manual resolver.

use std::fs;
use std::path::PathBuf;

use crate::consts::HENK_FILE_HEADER;
use crate::detect::{DetectionItem, Status};

pub fn probe(tld: &str) -> DetectionItem {
    let path = PathBuf::from(format!("/etc/resolver/{tld}"));
    if !path.exists() {
        return DetectionItem {
            name: "/etc/resolver/<tld>",
            status: Status::Ok,
            detail: format!("/etc/resolver/{tld} absent (will be created with sudo)"),
        };
    }
    match fs::read_to_string(&path) {
        Ok(contents) if contents.contains(HENK_FILE_HEADER) => DetectionItem {
            name: "/etc/resolver/<tld>",
            status: Status::Info,
            detail: format!("/etc/resolver/{tld} exists (managed by henk; reused)"),
        },
        Ok(_) => DetectionItem {
            name: "/etc/resolver/<tld>",
            status: Status::Block,
            detail: format!(
                "/etc/resolver/{tld} exists but isn't ours — pick a different TLD with --tld"
            ),
        },
        Err(_) => DetectionItem {
            name: "/etc/resolver/<tld>",
            status: Status::Warn,
            detail: format!("/etc/resolver/{tld} exists but is unreadable"),
        },
    }
}
